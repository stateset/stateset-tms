use super::*;
use crate::{
    auth::{Principal, Role},
    finance_read_tests::fixture,
    storage_fixture,
};
use std::time::Duration;
fn principal() -> Principal {
    Principal {
        tenant_id: "console".into(),
        subject: "user:console".into(),
        role: Role::Viewer,
        counterparty_id: None,
    }
}
fn query(value: Value) -> Query {
    serde_json::from_value(value).expect("query")
}
fn fixture_large(count: usize) -> Workspace {
    let mut w = fixture();
    let bill = w.bills[0].clone();
    let settlement = w.settlements[0].clone();
    w.bills.clear();
    w.settlements.clear();
    w.cash_entries.clear();
    w.credits.clear();
    w.carrier_credit_applications.clear();
    for n in 0..count {
        let mut b = bill.clone();
        b.id = format!("bill-{n:05}");
        b.number = format!("INV-{n:05}");
        b.snapshot.amount_cents = match n % 5 {
            0 => 0,
            1 => -1,
            _ => 100,
        };
        b.customer_id = "Zürich & literal %_".into();
        w.bills.push(b);
        let mut s = settlement.clone();
        s.id = format!("settlement-{n:05}");
        w.settlements.push(s);
        let out=serde_json::from_value(json!({"id":format!("same-{n:05}"),"load_id":format!("load-{n:05}"),"destination":"Zürich","status":if n%5==0{"failed"}else{"sent"},"method":"POST","path":"/owned","payload":{"private":"PRIVATE-PAYLOAD-CANARY"},"lease":"PRIVATE-LEASE-CANARY","response":"PRIVATE-RESPONSE-CANARY","last_error":"Quoted \"gateway\" diagnostic\\path\nline two ♥"})).expect("out");
        w.outbox.push(out);
        w.inbox.push(serde_json::from_value(json!({"id":format!("same-{n:05}"),"document_id":format!("DOC-{n:05}"),"delivery_ids":[],"key_version":"PRIVATE-KEY-CANARY","notification":{},"original_body":"PRIVATE-BODY-CANARY","fingerprint":"owned","status":"held","attempts":0,"total_attempts":0,"history":[],"received_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"})).expect("in"));
    }
    w
}
#[test]
fn index_projection_preserves_literal_search_values_exact_money_and_private_field_omission() {
    let w = fixture_large(3);
    let p = Projection::build(&w);
    assert_eq!(p.rows.len(), 12);
    assert_eq!(p.errors, json!({}));
    let raw = serde_json::to_value(&p.rows).expect("json");
    assert!(!raw.to_string().contains("PRIVATE-"));
    let row = raw
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["record_key"] == "bill-00001")
        .expect("bill");
    assert!(row["search_values"]
        .as_array()
        .expect("values")
        .contains(&json!("-0.01")));
    assert!(row["search_values"]
        .as_array()
        .expect("values")
        .contains(&json!("zürich & literal %_")));
    assert!(!row["search_values"]
        .as_array()
        .expect("values")
        .contains(&json!("customer_id")));
}
#[test]
fn cursor_versions_and_sql_shapes_are_explicit_and_bound() {
    assert!(cursor(&query(json!({"view":"billing","cursor":hex::encode(br#"{"version":1,"scope":"legacy","after":"x"}"#)}))).is_err());
    assert_eq!(QUERIES.len(), 16);
    for (mask, sql) in QUERIES
        .iter()
        .enumerate()
        .chain(SEARCH_QUERIES.iter().enumerate())
    {
        assert_eq!(sql.contains("s.balance=$3"), mask & 1 != 0);
        assert_eq!(sql.contains("s.direction=$4"), mask & 2 != 0);
        assert_eq!(sql.contains("s.attention=true"), mask & 4 != 0);
        assert_eq!(sql.contains("s.overdue=true"), mask & 8 != 0);
        assert!(
            sql.contains("LIMIT $7")
                && sql.contains("s.record_key>$6")
                && !sql.contains("/*FILTERS*/")
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_console_search_shares_one_match_scan_and_bounds_summary_fetches() {
    let f = Database::new().await;
    let mut w = fixture_large(5000);
    w.outbox.clear();
    w.inbox.clear();
    f.seed(&w).await;
    f.page(json!({"view":"billing"})).await;
    let before = storage_fixture::physical(&f.pool, "console")
        .await
        .expect("originals");
    sqlx::query("ANALYZE freight_console_records")
        .execute(&f.pool)
        .await
        .expect("statistics");
    fn nodes<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
        out.push(v);
        if let Some(children) = v["Plans"].as_array() {
            for child in children {
                nodes(child, out);
            }
        }
    }
    async fn result(conn: &mut sqlx::PgConnection, sql: &str) -> Value {
        let r = sqlx::query(sql).fetch_one(conn).await.expect("actual page");
        json!({"ready":r.get::<bool,_>("ready"),"finance":r.get::<String,_>("finance"),
            "errors":r.get::<String,_>("errors"),"total":r.get::<i64,_>("total"),
            "identities":r.get::<String,_>("identities"),"anchor":r.get::<bool,_>("anchor"),
            "rows":r.get::<String,_>("rows")})
    }
    for mode in ["force_generic_plan", "force_custom_plan", "auto"] {
        let mut conn = f.pool.acquire().await.expect("owned connection");
        sqlx::raw_sql(&format!("SET plan_cache_mode={mode}; PREPARE search_before(text,text,text,text,text,text,bigint,text) AS {}; PREPARE search_after(text,text,text,text,text,text,bigint,text) AS {}", QUERIES[0], SEARCH_QUERIES[0]))
            .execute(&mut *conn).await.expect("prepare exact statements");
        for (label, term, anchor) in [
            ("broad-first", "zürich", ""),
            ("broad-late", "zürich", "bill-04000"),
            ("rare", "inv-04999", ""),
            ("absent", "no-exposed-value-matches", ""),
        ] {
            let args = format!(
                "('console','billing','all','all','{term}','{anchor}',51,'{}')",
                Utc::now().date_naive()
            );
            let old = format!("EXECUTE search_before{args}");
            let new = format!("EXECUTE search_after{args}");
            let expected = result(&mut conn, &old).await;
            assert_eq!(result(&mut conn, &new).await, expected, "{mode}/{label}");
            // Alternating order retains cache/scheduler drift and unfavorable
            // samples. These are finite shared-host SQL observations, not SLOs.
            for iteration in 0..6 {
                let order = if iteration % 2 == 0 {
                    [("before", &old), ("after", &new)]
                } else {
                    [("after", &new), ("before", &old)]
                };
                for (variant, statement) in order {
                    let plan: Value = sqlx::query_scalar(&format!(
                        "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {statement}"
                    ))
                    .fetch_one(&mut *conn)
                    .await
                    .expect("observed execution");
                    if variant == "after" {
                        let mut all = Vec::new();
                        nodes(&plan[0]["Plan"], &mut all);
                        let searches: Vec<_> = all
                            .iter()
                            .filter(|n| n["Function Name"] == "unnest")
                            .collect();
                        assert_eq!(searches.len(), 1, "one exposed-value predicate: {plan}");
                        assert_eq!(
                            searches[0]["Actual Loops"], 5000,
                            "one search per candidate: {plan}"
                        );
                        let summary: Vec<_> = all
                            .iter()
                            .filter(|n| {
                                n["Relation Name"] == "freight_console_records" && n["Alias"] == "r"
                            })
                            .collect();
                        assert_eq!(summary.len(), 1, "one summary lookup branch: {plan}");
                        assert!(
                            summary[0]["Actual Loops"].as_u64().expect("lookup count") <= 51,
                            "{plan}"
                        );
                        assert!(
                            summary[0]["Actual Rows"].as_u64().expect("lookup rows") <= 1,
                            "{plan}"
                        );
                        assert!(!all
                            .iter()
                            .any(|n| n["Relation Name"] == "freight_workspaces"
                                || n["Relation Name"] == "freight_workspace_blocks"));
                    }
                    println!(
                        "CONSOLE_SEARCH_PROFILE {}",
                        json!({"mode":mode,"case":label,"variant":variant,"iteration":iteration,"records":5000,"plan":plan})
                    );
                }
            }
        }
        sqlx::raw_sql("DEALLOCATE search_before; DEALLOCATE search_after")
            .execute(&mut *conn)
            .await
            .expect("release prepared statements");
    }
    assert_eq!(
        storage_fixture::physical(&f.pool, "console")
            .await
            .expect("preserved originals"),
        before
    );
    f.clean().await;
}
struct Database {
    store: Store,
    pool: PgPool,
    admin: PgPool,
    schema: String,
    url: String,
}
impl Database {
    async fn new() -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("console_index_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("url");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
        url.query_pairs_mut()
            .append_pair("application_name", &schema);
        let store = Store::new(Some(url.as_str())).await.expect("migrations");
        let pool = store.pool.as_ref().expect("pool").clone();
        Self {
            store,
            pool,
            admin,
            schema,
            url: url.to_string(),
        }
    }
    async fn seed(&self, w: &Workspace) {
        storage_fixture::put_workspace(&self.pool, "console", w)
            .await
            .expect("source");
    }
    async fn page(&self, q: Value) -> Value {
        self.store
            .console_page("console", &principal(), query(q))
            .await
            .expect("page")
    }
    async fn clean(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .expect("cleanup");
        self.admin.close().await;
    }
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_console_index_matches_native_pages_filters_money_cursor_isolation_and_cold_rebuilds(
) {
    let f = Database::new().await;
    let mut w = fixture_large(123);
    f.seed(&w).await;
    let before = storage_fixture::physical(&f.pool, "console")
        .await
        .expect("physical");
    // Restore the preceding minor schema, then apply migrations 030 and 031.
    sqlx::raw_sql("DROP TRIGGER freight_integration_hint_invalidation ON freight_workspaces; DROP TRIGGER freight_integration_wakeups ON freight_workspaces; CREATE TRIGGER freight_integration_wakeups BEFORE INSERT OR UPDATE OF data,inbox_due_at,edi_due_at,sync_due_at ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION freight_refresh_integration_wakeups(); DROP TRIGGER freight_console_header ON freight_workspaces; DROP TRIGGER freight_console_blocks ON freight_workspace_blocks; DROP TRIGGER freight_console_truncate ON freight_workspace_blocks; DROP TABLE freight_console_records; DROP TABLE freight_console_state; DROP FUNCTION freight_invalidate_console(); DELETE FROM freight_schema_migrations WHERE version>=30")
        .execute(&f.pool).await.expect("preceding migration fixture");
    let upgraded = Store::new(Some(&f.url))
        .await
        .expect("upgrade 029 to current");
    upgraded.pool.as_ref().expect("upgraded pool").close().await;
    assert_eq!(
        storage_fixture::physical(&f.pool, "console")
            .await
            .expect("migration preservation"),
        before
    );

    let mut cases = Vec::new();
    for view in ["billing", "payables", "integrations"] {
        cases.push(json!({"view":view,"limit":50}));
    }
    for balance in ["open", "credits", "zero", "unknown", "overdue"] {
        cases.push(json!({"view":"billing","balance":balance,"limit":7}));
        cases.push(json!({"view":"billing","balance":balance,"q":" ZÜRICH ","limit":7}));
    }
    for direction in ["all", "outbound", "inbound"] {
        for attention in [false, true] {
            cases.push(json!({"view":"integrations","direction":direction,"attention":attention,"limit":7}));
            cases.push(json!({"view":"integrations","direction":direction,"attention":attention,"q":"same-","limit":7}));
        }
    }
    for q in [
        " ZÜRICH ",
        "literal %_",
        "-0.01",
        "DOC-00122",
        "EDI callback",
        "customer_id",
        "PRIVATE-PAYLOAD-CANARY",
        "%_",
        "' OR TRUE --",
    ] {
        for view in ["billing", "payables", "integrations"] {
            cases.push(json!({"view":view,"q":q,"limit":13}));
        }
    }
    let mut reads = 0;
    for mut q in cases {
        for _ in 0..3 {
            let expected = w
                .console_page("console", &principal(), query(q.clone()))
                .expect("native");
            let actual = f.page(q.clone()).await;
            assert_eq!(actual, expected, "{q}");
            reads += 1;
            if actual["next_cursor"].is_null() {
                break;
            }
            q["cursor"] = actual["next_cursor"].clone();
        }
    }
    assert_eq!(
        storage_fixture::physical(&f.pool, "console")
            .await
            .expect("unchanged"),
        before
    );
    let first = f.page(json!({"view":"billing","limit":2})).await;
    let cursor = first["next_cursor"].clone();
    let mut actor = principal();
    actor.subject = "user:other".into();
    assert!(f
        .store
        .console_page(
            "console",
            &actor,
            query(json!({"view":"billing","limit":2,"cursor":cursor}))
        )
        .await
        .is_err());
    assert!(f
        .store
        .console_page(
            "other",
            &principal(),
            query(json!({"view":"billing","limit":2,"cursor":cursor}))
        )
        .await
        .is_err());
    assert_eq!(
        f.store
            .console_page(
                "missing",
                &principal(),
                query(json!({"view":"integrations"}))
            )
            .await
            .expect("missing")["total"],
        0
    );
    sqlx::query("UPDATE freight_console_state SET as_of=as_of-1")
        .execute(&f.pool)
        .await
        .expect("UTC expiry");
    assert_eq!(f.page(json!({"view":"billing","limit":2})).await, first);
    // Rebuilds preserve original metadata, receipts, source bytes and timestamps.
    assert_eq!(
        storage_fixture::physical(&f.pool, "console")
            .await
            .expect("unchanged"),
        before
    );
    f.store
        .update("console", |w| {
            w.bills[2].snapshot.amount_cents = 43;
            Ok(json!({}))
        })
        .await
        .expect("native command");
    w.bills[2].snapshot.amount_cents = 43;
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT ready FROM freight_console_state WHERE tenant_id='console'"
    )
    .fetch_one(&f.pool)
    .await
    .expect("invalidated"));
    assert_eq!(
        f.page(json!({"view":"billing","limit":2,"cursor":cursor}))
            .await,
        w.console_page(
            "console",
            &principal(),
            query(json!({"view":"billing","limit":2,"cursor":cursor}))
        )
        .expect("fresh values")
    );
    w.bills.remove(0);
    f.seed(&w).await;
    assert!(f
        .store
        .console_page(
            "console",
            &principal(),
            query(json!({"view":"billing","limit":2,"cursor":cursor}))
        )
        .await
        .is_err());
    w.bills[0].snapshot.amount_cents = i64::MAX;
    f.seed(&w).await;
    assert!(f
        .store
        .console_page("console", &principal(), query(json!({"view":"billing"})))
        .await
        .is_err());
    assert_eq!(f.page(json!({"view":"integrations"})).await["total"], 246);
    println!("PASS indexed console native parity: {reads} pages plus UTC expiry, original-byte preservation, native-write invalidation, actor/tenant isolation and overflow quarantine");
    f.clean().await;
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_warm_console_pages_progress_while_workspace_capacity_is_held_and_cold_reads_wait()
{
    let mut f = Database::new().await;
    f.seed(&fixture_large(60)).await;
    f.page(json!({"view":"billing"})).await;
    f.store.read_slots = Arc::new(Semaphore::new(1));
    let read = f
        .store
        .read_slots
        .clone()
        .acquire_owned()
        .await
        .expect("hold snapshot");
    let warm = tokio::time::timeout(
        Duration::from_millis(500),
        f.store.console_page(
            "console",
            &principal(),
            query(json!({"view":"billing","balance":"credits"})),
        ),
    )
    .await
    .expect("warm independent")
    .expect("warm");
    assert!(warm["total"].as_i64().expect("count") > 0);
    sqlx::query("UPDATE freight_console_state SET ready=false")
        .execute(&f.pool)
        .await
        .expect("cold");
    let store = f.store.clone();
    let cold = tokio::spawn(async move {
        store
            .console_page("console", &principal(), query(json!({"view":"billing"})))
            .await
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!cold.is_finished());
    let free = tokio::time::timeout(Duration::from_millis(200), f.pool.acquire())
        .await
        .expect("cold waits before DB")
        .expect("connection");
    drop(free);
    cold.abort();
    assert!(cold.await.expect_err("abandoned caller").is_cancelled());
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            f.store.catalog_admission.acquire("other")
        )
        .await
        .is_err(),
        "running cold read retains page admission"
    );
    drop(read);
    let lane = tokio::time::timeout(
        Duration::from_secs(5),
        f.store.catalog_admission.acquire("after-cold"),
    )
    .await
    .expect("rebuild completes")
    .expect("lane");
    drop(lane);
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT ready FROM freight_console_state WHERE tenant_id='console'"
    )
    .fetch_one(&f.pool)
    .await
    .expect("published after caller abort"));
    f.clean().await;
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_console_rebuild_rechecks_cache_under_header_lock_before_reading_blocks() {
    let f = Database::new().await;
    f.seed(&fixture_large(60)).await;
    let expected = f.page(json!({"view":"billing"})).await;
    let before = storage_fixture::physical(&f.pool, "console")
        .await
        .expect("originals");
    // A caller can have observed a cold cache before another process publishes.
    // The recheck must lock the tenant but must not transfer its originals again.
    let mut blocks = f.admin.begin().await.expect("block gate");
    sqlx::query(&format!(
        "LOCK TABLE {}.freight_workspace_blocks IN ACCESS EXCLUSIVE MODE",
        f.schema
    ))
    .execute(&mut *blocks)
    .await
    .expect("hold original relation");
    let permit = f
        .store
        .catalog_admission
        .acquire("console")
        .await
        .expect("page");
    assert!(tokio::time::timeout(
        Duration::from_secs(2),
        f.store.rebuild_console("console", permit),
    )
    .await
    .expect("published cache must not wait for original blocks")
    .expect("published cache"));
    assert_eq!(f.page(json!({"view":"billing"})).await, expected);
    blocks.rollback().await.expect("release original relation");

    // A genuinely invalid cache still reads originals. Witness that exact SQL
    // waiting on the relation lock before releasing it, rather than sleeping.
    sqlx::query("UPDATE freight_console_state SET ready=false WHERE tenant_id='console'")
        .execute(&f.pool)
        .await
        .expect("invalidate");
    let mut blocks = f.admin.begin().await.expect("cold block gate");
    sqlx::query(&format!(
        "LOCK TABLE {}.freight_workspace_blocks IN ACCESS EXCLUSIVE MODE",
        f.schema
    ))
    .execute(&mut *blocks)
    .await
    .expect("hold cold originals");
    let permit = f
        .store
        .catalog_admission
        .acquire("console")
        .await
        .expect("cold page");
    let store = f.store.clone();
    let cold = tokio::spawn(async move { store.rebuild_console("console", permit).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND query LIKE 'SELECT root,key,payload::text FROM freight_workspace_blocks%')")
                .bind(&f.schema).fetch_one(&f.admin).await.expect("witness original read");
            if waiting { break; }
            assert!(!cold.is_finished(), "cold rebuild must read originals");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("observed original-block query");
    blocks.rollback().await.expect("unblock rebuild");
    assert!(cold.await.expect("owned rebuild").expect("rebuilt"));
    assert_eq!(f.page(json!({"view":"billing"})).await, expected);
    assert_eq!(
        storage_fixture::physical(&f.pool, "console")
            .await
            .expect("retained originals"),
        before
    );
    f.clean().await;
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_console_projection_uses_indexed_keyset_pages_on_large_tenants() {
    let f = Database::new().await;
    let mut w = fixture_large(5000);
    // This qualifies financial index plans. Large integration-queue writes
    // exercise separate canonical wakeup triggers and are not this profile.
    w.outbox.clear();
    w.inbox.clear();
    f.seed(&w).await;
    let first = f.page(json!({"view":"billing","limit":50})).await;
    assert_eq!(first["total"], 5000);
    assert_eq!(first["bills"].as_array().expect("bound").len(), 50);
    sqlx::query("ANALYZE freight_console_records")
        .execute(&f.pool)
        .await
        .expect("statistics");
    // Observe actual page branches under generic/custom/automatic planning.
    for mode in ["force_generic_plan", "force_custom_plan", "auto"] {
        let mut conn = f.pool.acquire().await.expect("plan");
        let q = query(json!({"view":"billing","balance":"credits"}));
        let name = format!("console_{mode}");
        sqlx::raw_sql(&format!("SET plan_cache_mode={mode}; PREPARE {name}(text,text,text,text,text,text,bigint,text) AS {}",QUERIES[mask(&q)]))
            .execute(&mut *conn).await.expect("prepare actual SELECT");
        let execute = format!(
            "EXECUTE {name}('console','billing','credits','all','','bill-02000',51,'{}')",
            Utc::now().date_naive()
        );
        if mode == "auto" {
            for _ in 0..6 {
                sqlx::raw_sql(&execute)
                    .execute(&mut *conn)
                    .await
                    .expect("auto plan history");
            }
        }
        let plan: Value =
            sqlx::query_scalar(&format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {execute}"))
                .fetch_one(&mut *conn)
                .await
                .expect("actual prepared plan");
        fn inspect(v: &Value, found: &mut bool) {
            if v["Node Type"] == "Limit" {
                assert!(v["Actual Rows"].as_u64().expect("rows") <= 51);
                if let Some(child) = v["Plans"].as_array().and_then(|a| a.first()) {
                    let mut scan = child;
                    // The ready/date guard is a one-time Result above the
                    // range scan in generic plans; it neither sorts nor scans.
                    while scan["Node Type"] == "Result" {
                        let children = scan["Plans"].as_array().expect("transparent Result child");
                        assert_eq!(children.len(), 1);
                        scan = &children[0];
                    }
                    assert!(
                        matches!(
                            scan["Node Type"].as_str(),
                            Some("Index Scan" | "Index Only Scan")
                        ),
                        "{scan}"
                    );
                    assert_eq!(scan["Relation Name"], "freight_console_records");
                    assert!(
                        scan["Index Cond"]
                            .as_str()
                            .is_some_and(|s| s.contains("record_key >") && s.contains("balance =")),
                        "{scan}"
                    );
                    assert!(scan["Actual Rows"].as_u64().expect("scan bound") <= 51);
                    *found = true;
                }
            }
            if let Some(nodes) = v["Plans"].as_array() {
                for n in nodes {
                    inspect(n, found);
                }
            }
        }
        let mut found = false;
        inspect(&plan[0]["Plan"], &mut found);
        assert!(found, "keyset Limit/index branch must be visible");
        println!("PASS indexed console 5000-record plan {mode}: {}", plan);
    }
    let p = f.page(json!({"view":"billing","q":"INV-04999"})).await;
    assert_eq!(p["total"], 1);
    assert_eq!(p["bills"][0]["id"], "bill-04999");
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_abandoned_console_sql_retains_page_admission_until_database_completion() {
    let f = Database::new().await;
    f.seed(&fixture_large(20)).await;
    f.page(json!({"view":"billing"})).await;
    let before = storage_fixture::physical(&f.pool, "console")
        .await
        .expect("source");
    let mut blocker = f.pool.begin().await.expect("blocker");
    sqlx::query("LOCK freight_console_state IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("owned lock");
    let store = f.store.clone();
    let caller = tokio::spawn(async move {
        store
            .console_page("console", &principal(), query(json!({"view":"billing"})))
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND query LIKE '%facts.total%')").bind(&f.schema).fetch_one(&f.admin).await.expect("actual SQL wait");
        if waiting {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    caller.abort();
    assert!(caller.await.expect_err("abandoned caller").is_cancelled());
    assert!(tokio::time::timeout(
        Duration::from_millis(30),
        f.store.catalog_admission.acquire("other")
    )
    .await
    .is_err());
    blocker.rollback().await.expect("release owned lock");
    let permit = tokio::time::timeout(
        Duration::from_secs(3),
        f.store.catalog_admission.acquire("after"),
    )
    .await
    .expect("SQL completion")
    .expect("permit");
    drop(permit);
    assert_eq!(
        storage_fixture::physical(&f.pool, "console")
            .await
            .expect("unchanged"),
        before
    );
    f.page(json!({"view":"billing"})).await;
    f.clean().await;
}
