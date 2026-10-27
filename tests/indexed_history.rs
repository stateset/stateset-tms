#[path = "support/canonical.rs"]
mod canonical;
use chrono::Utc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool, Row};
use stateset_freight::{
    domain::{self, Event},
    history::Query,
    store::Store,
};

const PREVIOUS: [&str; 23] = [
    include_str!("../migrations/001_workspaces.sql"),
    include_str!("../migrations/002_documents.sql"),
    include_str!("../migrations/003_storage_format.sql"),
    include_str!("../migrations/004_cancellation_fees.sql"),
    include_str!("../migrations/005_quotes.sql"),
    include_str!("../migrations/006_source_reviews.sql"),
    include_str!("../migrations/007_quote_amendments.sql"),
    include_str!("../migrations/008_customer_refunds.sql"),
    include_str!("../migrations/009_rate_confirmations.sql"),
    include_str!("../migrations/010_partner_portal.sql"),
    include_str!("../migrations/011_carrier_submissions.sql"),
    include_str!("../migrations/012_customer_quote_portal.sql"),
    include_str!("../migrations/013_document_sharing.sql"),
    include_str!("../migrations/014_carrier_release_portal.sql"),
    include_str!("../migrations/015_carrier_contracts.sql"),
    include_str!("../migrations/016_multi_stop.sql"),
    include_str!("../migrations/017_execution_chronology.sql"),
    include_str!("../migrations/018_movement_corrections.sql"),
    include_str!("../migrations/019_operations_work.sql"),
    include_str!("../migrations/020_integration_wakeups.sql"),
    include_str!("../migrations/021_load_catalog.sql"),
    include_str!("../migrations/022_incremental_load_catalog.sql"),
    include_str!("../migrations/023_save_resolution.sql"),
];
struct Database {
    admin: PgPool,
    pool: PgPool,
    url: String,
    schema: String,
}
impl Database {
    async fn new(previous: bool) -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("indexed_history_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("url");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema},pg_catalog"))
            .append_pair("application_name", &schema);
        let pool = PgPool::connect(url.as_str()).await.expect("pool");
        if previous {
            let mut tx = pool.begin().await.expect("previous transaction");
            sqlx::raw_sql("CREATE TABLE freight_schema_migrations(version integer PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())").execute(&mut *tx).await.expect("ledger");
            for (i, sql) in PREVIOUS.iter().enumerate() {
                sqlx::raw_sql(sql)
                    .execute(&mut *tx)
                    .await
                    .expect("previous migration");
                sqlx::query(
                    "INSERT INTO freight_schema_migrations(version,checksum) VALUES($1,$2)",
                )
                .bind(i as i32 + 1)
                .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
                .execute(&mut *tx)
                .await
                .expect("checksum");
            }
            tx.commit().await.expect("old schema");
        }
        Self {
            admin,
            pool,
            url: url.to_string(),
            schema,
        }
    }
    async fn clean(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .expect("cleanup");
        self.admin.close().await;
    }
    async fn raw(&self) -> Value {
        sqlx::query_scalar("SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id='demo'")
            .fetch_one(&self.pool)
            .await
            .expect("source")
    }
}
fn fixture() -> Value {
    let mut w = domain::demo();
    for n in 0..260 {
        w.events.push(Event {
            id: format!("owned-original-{n}"),
            entity_id: if n % 2 == 0 {
                "load_1".into()
            } else {
                "other-fact".into()
            },
            kind: "owned.fixture".into(),
            message: format!("Original fact {n}"),
            occurred_at: Utc::now(),
            actor: "owned:operator".into(),
            details: Some(json!({"ordinal":n,"original":"do not replace"})),
        });
    }
    let mut v = json!(w);
    v["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    v
}
fn query(v: Value) -> Query {
    serde_json::from_value(v).expect("query")
}
async fn all_events(s: &Store, load: Option<&str>) -> Vec<Value> {
    let mut cursor = Value::Null;
    let mut out = vec![];
    loop {
        let mut q = json!({"kind":"events","limit":17});
        if let Some(id) = load {
            q["load_id"] = json!(id);
        }
        if !cursor.is_null() {
            q["cursor"] = cursor;
        }
        let page = s
            .history_page("demo", query(q))
            .await
            .expect("original page");
        out.extend(
            page["items"]
                .as_array()
                .expect("items")
                .iter()
                .map(|i| i["record"].clone()),
        );
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    out
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_history_minor_upgrade_preserves_source_and_canonical_upgrade_fences_old_prepared_writer_then_reconnects(
) {
    let db = Database::new(true).await;
    let data = fixture();
    let mut previous = data.clone();
    previous["_freight_format"] = json!(18);
    sqlx::query("INSERT INTO freight_workspaces(tenant_id,data,updated_at) VALUES('demo',$1,'2026-01-01T00:00:00Z')").bind(&previous).execute(&db.pool).await.expect("old source");
    let before:(String,String,String)=sqlx::query_as("SELECT xmin::text,updated_at::text,md5(data::text) FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("physical before");
    let mut old = db.pool.acquire().await.expect("old connection");
    sqlx::query("PREPARE owned_old_writer(jsonb) AS UPDATE freight_workspaces SET data=$1,updated_at=now() WHERE tenant_id='demo'").execute(&mut *old).await.expect("old prepared plan");
    canonical::minor_migration(
        &db.pool,
        24,
        include_str!("../migrations/024_indexed_history.sql"),
    )
    .await
    .expect("history-only upgrade");
    let minor_after: (String,String,String) = sqlx::query_as("SELECT xmin::text,updated_at::text,md5(data::text) FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("minor source after");
    assert_eq!(
        before, minor_after,
        "minor history migration must not rewrite source"
    );
    let literal = previous.to_string().replace('\'', "''");
    sqlx::raw_sql(&format!("EXECUTE owned_old_writer('{literal}'::jsonb)"))
        .execute(&mut *old)
        .await
        .expect("minor migration keeps prepared writer");
    sqlx::query("SELECT freight_ensure_history('demo')")
        .execute(&db.pool)
        .await
        .expect("warm history after old writer");
    let old_revision: i64 = sqlx::query_scalar(
        "SELECT history_revision FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("old writer revision");
    assert_eq!(old_revision, 1);
    let stamp: (String, i64) = sqlx::query_as(
        "SELECT updated_at::text,history_revision FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("original metadata");
    let (one, two) = tokio::join!(Store::new(Some(&db.url)), Store::new(Some(&db.url)));
    let store = one.expect("upgrader");
    drop(two.expect("second upgrader"));
    let after: (String, i64) = sqlx::query_as(
        "SELECT updated_at::text,history_revision FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("canonical metadata");
    assert_eq!(
        stamp, after,
        "canonical backfill preserves source time and revision"
    );
    assert_eq!(db.raw().await, data);
    let expected: Vec<_> = data["events"]
        .as_array()
        .expect("events")
        .iter()
        .rev()
        .cloned()
        .collect();
    assert_eq!(all_events(&store, None).await, expected);
    assert_eq!(
        all_events(&store, Some("load_1")).await,
        expected
            .iter()
            .filter(|e| e["entity_id"] == "load_1")
            .cloned()
            .collect::<Vec<_>>()
    );
    // Execute the actual already-prepared preceding-schema statement. SQL text
    // is an owned synthetic fixture and quoted as a literal, not a user value.
    let mut changed = data.clone();
    changed["events"].as_array_mut().expect("events").pop();
    let literal = changed.to_string().replace('\'', "''");
    let refusal = sqlx::raw_sql(&format!("EXECUTE owned_old_writer('{literal}'::jsonb)"))
        .execute(&mut *old)
        .await
        .expect_err("major upgrade fences aggregate writer even with current marker");
    assert_eq!(
        refusal
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514")
    );
    canonical::put(&db.pool, "demo", &changed)
        .await
        .expect("current canonical source update");
    assert_eq!(all_events(&store, None).await.len() + 1, expected.len());
    let row=sqlx::query("SELECT w.history_revision,s.source_revision FROM freight_workspaces w JOIN freight_history_state s USING(tenant_id) WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("generation");
    assert_eq!(
        row.get::<i64, _>("history_revision"),
        row.get::<i64, _>("source_revision")
    );
    assert!(row.get::<i64, _>("history_revision") > old_revision);
    drop(old);
    drop(store);
    let connected = Store::new(Some(&db.url)).await.expect("restart");
    assert_eq!(all_events(&connected, None).await.len() + 1, expected.len());
    drop(connected);
    db.clean().await;
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_history_commit_visibility_stale_projection_damage_and_explicit_repair() {
    let db = Database::new(false).await;
    let store = Store::new(Some(&db.url)).await.expect("store");
    let data = fixture();
    canonical::put(&db.pool, "demo", &data)
        .await
        .expect("source");
    let original = all_events(&store, None).await;
    let mut changed = data.clone();
    changed["events"].as_array_mut().expect("events").pop();
    let mut tx = db.pool.begin().await.expect("pending source");
    canonical::put_tx(&mut tx, "demo", &changed)
        .await
        .expect("uncommitted change");
    assert_eq!(
        all_events(&store, None).await,
        original,
        "other readers see committed source/index together"
    );
    tx.rollback().await.expect("rollback");
    assert_eq!(all_events(&store, None).await, original);
    let before: String =
        sqlx::query_scalar("SELECT md5(freight_workspace_originals(tenant_id)::text) FROM freight_workspaces WHERE tenant_id='demo'")
            .fetch_one(&db.pool)
            .await
            .expect("original checksum");
    sqlx::query("DELETE FROM freight_history_scopes WHERE tenant_id='demo' AND kind='events' AND scope_id='other-fact'").execute(&db.pool).await.expect("owned damage");
    assert!(
        store
            .history_page("demo", query(json!({"kind":"events"})))
            .await
            .is_err(),
        "missing originals cannot look like a complete smaller collection"
    );
    sqlx::query("SELECT freight_rebuild_history('demo')")
        .execute(&db.pool)
        .await
        .expect("repair");
    assert_eq!(all_events(&store, None).await, original);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT md5(freight_workspace_originals(tenant_id)::text) FROM freight_workspaces WHERE tenant_id='demo'"
        )
        .fetch_one(&db.pool)
        .await
        .expect("retained checksum"),
        before
    );
    // Only the projection trigger is disabled in this owned schema. The
    // independent source revision still advances and prevents stale reads.
    let mut tx = db.pool.begin().await.expect("stale projection fault");
    sqlx::query("ALTER TABLE freight_workspaces DISABLE TRIGGER freight_history_refresh")
        .execute(&mut *tx)
        .await
        .expect("owned fault");
    canonical::put_tx(&mut tx, "demo", &changed)
        .await
        .expect("source update");
    sqlx::query("SET CONSTRAINTS freight_history_refresh IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("finish fault transaction events before DDL");
    sqlx::query("ALTER TABLE freight_workspaces ENABLE TRIGGER freight_history_refresh")
        .execute(&mut *tx)
        .await
        .expect("restore trigger");
    tx.commit().await.expect("commit stale fixture");
    assert_eq!(
        all_events(&store, None).await.len() + 1,
        original.len(),
        "a stale source revision rebuilds before serving, even if invalidation was disabled"
    );
    sqlx::query("SELECT freight_rebuild_history('demo')")
        .execute(&db.pool)
        .await
        .expect("rebuild stale source");
    assert_eq!(all_events(&store, None).await.len() + 1, original.len());
    // Two updates defer projection to the final source revision, not an
    // intermediate snapshot; restore the complete original as the final row.
    let mut tx = db.pool.begin().await.expect("multi update");
    canonical::put_tx(&mut tx, "demo", &data)
        .await
        .expect("first");
    canonical::put_tx(&mut tx, "demo", &changed)
        .await
        .expect("second");
    tx.commit().await.expect("final projection");
    assert_eq!(all_events(&store, None).await.len() + 1, original.len());
    drop(store);
    db.clean().await;
}
fn plan_nodes<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
    out.push(node);
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            plan_nodes(child, out);
        }
    }
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_history_pages_cross_block_boundaries_and_refuse_a_forged_zero_ordinal() {
    let db = Database::new(false).await;
    let store = Store::new(Some(&db.url)).await.expect("store");
    let data = fixture();
    canonical::put(&db.pool, "demo", &data)
        .await
        .expect("originals");
    let events = data["events"].as_array().expect("events");
    for limit in [1, 17, 25, 64, 100] {
        for scoped in [false, true] {
            let mut q = json!({"kind":"events","limit":limit});
            if scoped {
                q["load_id"] = json!("load_1");
            }
            let page = store.history_page("demo", query(q)).await.expect("page");
            let expected: Vec<_> = events
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, record)| !scoped || record["entity_id"] == "load_1")
                .take(limit)
                .map(|(index, record)| json!({"position":index + 1,"record":record}))
                .collect();
            assert_eq!(page["items"], json!(expected));
            assert_eq!(page["captured_through"], json!(events.len()));
        }
    }
    assert_eq!(
        db.raw().await,
        data,
        "page and anchor reads preserve all originals"
    );

    // Damage both the original ordinal and its derived scope while preserving
    // counts. A matching payload in the first block must never authorize zero.
    let retained = events
        .iter()
        .find(|event| event["entity_id"] == "load_1")
        .expect("load history");
    let valid_position = events
        .iter()
        .enumerate()
        .find(|(index, event)| *index > 0 && *index < 64 && event["entity_id"] == "load_1")
        .map(|(index, _)| index as i64 + 1)
        .expect("same-block valid history");
    let mut tx = db.pool.begin().await.expect("owned corruption");
    // Bypass only this owned fixture's derived-data protections to reproduce
    // already-damaged storage. Restore guards before exercising the reader.
    sqlx::raw_sql("ALTER TABLE freight_workspaces DISABLE TRIGGER freight_history_refresh; ALTER TABLE freight_history_scopes DROP CONSTRAINT freight_history_scopes_positions_check")
        .execute(&mut *tx).await.expect("owned corruption guards");
    let changed = sqlx::query("UPDATE freight_workspace_blocks SET payload=jsonb_set(jsonb_set(payload,'{items,0,position}','0'::jsonb),'{items,0,payload}',$1) WHERE tenant_id='demo' AND root='events' AND payload #>> '{items,0,position}'='1'")
        .bind(retained).execute(&mut *tx).await.expect("zero original ordinal");
    assert_eq!(changed.rows_affected(), 1);
    sqlx::query("UPDATE freight_history_scopes SET positions=$1 WHERE tenant_id='demo' AND kind='events' AND scope_id='load_1'")
        .bind(vec![0i64,valid_position]).execute(&mut *tx).await.expect("forged scope");
    sqlx::query("UPDATE freight_history_state s SET ready=true,source_revision=w.history_revision FROM freight_workspaces w WHERE s.tenant_id=w.tenant_id AND s.tenant_id='demo'")
        .execute(&mut *tx).await.expect("forged projection generation");
    sqlx::raw_sql("ALTER TABLE freight_workspaces ENABLE TRIGGER freight_history_refresh; ALTER TABLE freight_history_scopes ADD CONSTRAINT freight_history_scopes_positions_check CHECK(freight_history_positions_valid(positions)) NOT VALID")
        .execute(&mut *tx).await.expect("restore fixture guards");
    tx.commit().await.expect("owned fixture commit");
    assert!(
        store
            .history_page(
                "demo",
                query(json!({"kind":"events","load_id":"load_1","limit":25}))
            )
            .await
            .is_err(),
        "zero is never an original ordinal, even with a matching record and forged projection"
    );
    drop(store);
    db.clean().await;
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_history_scope_plans_are_indexed_and_original_large_identities_stay_available() {
    let db = Database::new(false).await;
    let store = Store::new(Some(&db.url)).await.expect("store");
    let mut data = fixture();
    let sample = data["events"][0].clone();
    for n in 0..1800 {
        let mut e = sample.clone();
        e["id"] = json!(format!("unrelated-{n}"));
        e["entity_id"] = json!(format!("other-scope-{n:05}"));
        data["events"].as_array_mut().expect("events").push(e);
    }
    let mut large = sample;
    large["id"] = json!("original-".repeat(1200));
    large["entity_id"] = json!("outside-query-length-".repeat(40));
    data["events"]
        .as_array_mut()
        .expect("events")
        .push(large.clone());
    canonical::put(&db.pool, "demo", &data)
        .await
        .expect("original source");
    let cold = store
        .history_page("demo", query(json!({"kind":"events","limit":1})))
        .await
        .expect("cold index build");
    assert_eq!(
        cold["items"][0]["record"], large,
        "an oversized identity must not be truncated or make its original inaccessible globally"
    );
    let expected: Vec<_> = data["events"]
        .as_array()
        .expect("events")
        .iter()
        .rev()
        .filter(|e| e["entity_id"] == "load_1")
        .cloned()
        .collect();
    assert_eq!(all_events(&store, Some("load_1")).await, expected);
    sqlx::raw_sql("ANALYZE freight_history_scopes; ANALYZE freight_history_loads; ANALYZE freight_history_state; ANALYZE freight_history_collections;").execute(&db.pool).await.expect("statistics");
    let mut plans = vec![];
    for mode in ["force_generic_plan", "force_custom_plan", "auto"] {
        let mut conn = PgConnection::connect(&db.url)
            .await
            .expect("plan connection");
        sqlx::raw_sql(&format!("SET plan_cache_mode={mode}; PREPARE owned_history(text,text,text,bigint,bigint,bigint,text) AS {}",include_str!("../src/history_page.sql"))).execute(&mut conn).await.expect("prepare exact page SELECT");
        if mode == "auto" {
            for _ in 0..6 {
                sqlx::raw_sql("EXECUTE owned_history('demo','events','load_1',NULL,NULL,18,'21')")
                    .execute(&mut conn)
                    .await
                    .expect("warm");
            }
        }
        let plan:Value=sqlx::query_scalar("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE owned_history('demo','events','load_1',NULL,NULL,18,'21')").fetch_one(&mut conn).await.expect("plan");
        let mut nodes = vec![];
        plan_nodes(&plan[0]["Plan"], &mut nodes);
        let scopes: Vec<_> = nodes
            .iter()
            .filter(|n| n["Relation Name"] == "freight_history_scopes")
            .collect();
        assert!(!scopes.is_empty(), "scope relation absent: {plan}");
        for node in scopes {
            assert!(
                node["Node Type"] == "Index Scan" || node["Node Type"] == "Index Only Scan",
                "{mode}: {node}"
            );
            let condition = node["Index Cond"].as_str().expect("index condition");
            for column in ["tenant_id", "kind", "scope_id"] {
                assert!(condition.contains(column), "{mode}: {condition}");
            }
            assert!(
                node["Actual Rows"].as_u64().expect("rows") <= 1,
                "must not examine every unrelated scope"
            );
        }
        let blocks: Vec<_> = nodes
            .iter()
            .filter(|node| node["Relation Name"] == "freight_workspace_blocks")
            .collect();
        assert!(!blocks.is_empty(), "original block relation absent: {plan}");
        for node in blocks {
            let mut access_nodes = Vec::new();
            plan_nodes(node, &mut access_nodes);
            assert!(
                access_nodes
                    .iter()
                    .any(
                        |access| access["Index Cond"].as_str().is_some_and(|condition| [
                            "tenant_id",
                            "root",
                            "key"
                        ]
                        .iter()
                        .all(|column| condition.contains(column)))
                    ),
                "{mode}: original block access must constrain tenant, root and key range: {node}"
            );
        }
        plans.push(json!({"mode":mode,"plan":plan}));
    }
    if let Ok(path) = std::env::var("FREIGHT_INDEXED_HISTORY_PLAN_EVIDENCE") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"result":"pass","unrelated_scopes":1800,"plans":plans}),
            )
            .expect("report"),
        )
        .expect("owned report");
    }
    drop(store);
    db.clean().await;
}

async fn blocked_history(db: &Database, fragment: &str, expected: i64) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND position($2 IN query)>0")
            .bind(&db.schema).bind(fragment).fetch_one(&db.admin).await.expect("owned blocked readers");
        if count >= expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "history did not reach its controlled lock: {fragment}, {count}/{expected}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_history_snapshot_survives_interleaved_commit_and_retries_cold_conflicts_with_a_bound(
) {
    let db = Database::new(false).await;
    let store = Store::new(Some(&db.url)).await.expect("store");
    let original = fixture();
    canonical::put(&db.pool, "demo", &original)
        .await
        .expect("original source");
    all_events(&store, None).await;
    // Pause the real page SELECT after the warm readiness check, while a
    // different connection commits a source change and invalidates the index.
    let mut blocker = db.pool.begin().await.expect("page blocker");
    sqlx::query("LOCK TABLE freight_history_collections IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("hold only page metadata");
    let reader = {
        let store = store.clone();
        tokio::spawn(async move {
            store
                .history_page("demo", query(json!({"kind":"events","limit":100})))
                .await
        })
    };
    blocked_history(&db, "WITH head AS MATERIALIZED", 1).await;
    let mut changed = original.clone();
    changed["events"].as_array_mut().expect("events").pop();
    canonical::put(&db.pool, "demo", &changed)
        .await
        .expect("commit between readiness and selection");
    blocker.rollback().await.expect("release page");
    let page = reader
        .await
        .expect("reader task")
        .expect("coherent admitted snapshot");
    let expected: Vec<_> = original["events"]
        .as_array()
        .expect("events")
        .iter()
        .rev()
        .take(100)
        .cloned()
        .collect();
    let actual: Vec<_> = page["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["record"].clone())
        .collect();
    assert_eq!(
        actual, expected,
        "a later commit must not mix original source and cache generations"
    );
    assert_eq!(
        page["captured_through"],
        original["events"].as_array().expect("events").len()
    );
    assert_eq!(
        all_events(&store, None).await.len(),
        changed["events"].as_array().expect("events").len()
    );

    sqlx::query("UPDATE freight_workspaces SET data=data WHERE tenant_id='demo'")
        .execute(&db.pool)
        .await
        .expect("invalidate for competing cold readers");
    let before: (String, String, i64) = sqlx::query_as("SELECT md5(data::text),xmin::text,history_revision FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("source before rebuilds");
    // Local index preparation now shares a bounded tenant turn with writes.
    // Use independent Stores to preserve two genuinely competing database
    // builders, as separate serving instances can still race on this source.
    let other_store = Store::new(Some(&db.url)).await.expect("independent store");
    let mut blocker = db.pool.begin().await.expect("cold builder blocker");
    sqlx::query("SELECT 1 FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("hold tenant");
    let readers: Vec<_> = [store.clone(), other_store.clone()]
        .into_iter()
        .map(|store| {
            tokio::spawn(async move {
                store
                    .history_page("demo", query(json!({"kind":"events","limit":25})))
                    .await
            })
        })
        .collect();
    blocked_history(&db, "SELECT freight_ensure_history", 2).await;
    blocker
        .rollback()
        .await
        .expect("release competing cold builders");
    for reader in readers {
        let page = reader
            .await
            .expect("cold reader task")
            .expect("cold conflict restarts against a fresh snapshot");
        assert_eq!(
            page["items"][0]["record"],
            changed["events"]
                .as_array()
                .expect("events")
                .last()
                .expect("last original")
                .clone()
        );
    }
    let after: (String, String, i64) = sqlx::query_as("SELECT md5(data::text),xmin::text,history_revision FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("source after rebuilds");
    assert_eq!(
        before, after,
        "competing cache builders must not rewrite source"
    );

    // A nontransactional sequence observes actual attempts even as each
    // deliberately conflicting owned transaction rolls back.
    sqlx::raw_sql("CREATE SEQUENCE owned_history_conflicts; CREATE OR REPLACE FUNCTION freight_ensure_history(p_tenant text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('owned_history_conflicts'); RAISE EXCEPTION 'owned repeated conflict' USING ERRCODE='40001'; END $$;").execute(&db.pool).await.expect("owned persistent conflict");
    assert_eq!(
        store
            .history_page("demo", query(json!({"kind":"events"})))
            .await
            .expect_err("finite conflicts are retryable"),
        stateset_freight::store::DATABASE_BUSY
    );
    let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM owned_history_conflicts")
        .fetch_one(&db.pool)
        .await
        .expect("actual retry count");
    assert_eq!(attempts, 3, "serialization retry must remain bounded");
    assert_eq!(db.raw().await, changed);
    drop(other_store);
    drop(store);
    db.clean().await;
}
