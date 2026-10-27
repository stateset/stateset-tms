use serde_json::{json, value::RawValue};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use stateset_freight::{domain, store::Store};
use std::collections::BTreeMap;
#[path = "support/canonical.rs"]
mod canonical;

const LEGACY: [&str; 25] = [
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
    include_str!("../migrations/024_indexed_history.sql"),
    include_str!("../migrations/025_tender_deadlines.sql"),
];
struct Database {
    admin: PgPool,
    pool: PgPool,
    url: String,
    schema: String,
}
impl Database {
    async fn new() -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("canonical_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("url");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
        let pool = PgPool::connect(url.as_str()).await.expect("pool");
        Self {
            admin,
            pool,
            url: url.to_string(),
            schema,
        }
    }
    async fn legacy(&self) {
        let mut tx = self.pool.begin().await.expect("migration transaction");
        sqlx::raw_sql("CREATE TABLE freight_schema_migrations(version integer PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())").execute(&mut *tx).await.expect("ledger");
        for (n, sql) in LEGACY.iter().enumerate() {
            sqlx::raw_sql(sql)
                .execute(&mut *tx)
                .await
                .expect("legacy migration");
            sqlx::query("INSERT INTO freight_schema_migrations(version,checksum) VALUES($1,$2)")
                .bind(n as i32 + 1)
                .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
                .execute(&mut *tx)
                .await
                .expect("checksum");
        }
        tx.commit().await.expect("legacy commit");
    }
    async fn put_legacy(&self, tenant: &str, raw: Box<RawValue>) {
        sqlx::query("INSERT INTO freight_workspaces(tenant_id,data,updated_at) VALUES($1,$2,'2026-01-02T03:04:05Z')")
            .bind(tenant).bind(sqlx::types::Json(raw)).execute(&self.pool).await.expect("original source");
    }
    async fn canonical19(&self) {
        // Owned historical fixture: real migration026 SQL and codec v1 packing.
        // Current Store startup must perform027 itself, never this helper.
        let sql = include_str!("../migrations/026_canonical_blocks.sql");
        let (setup, finish) = sql
            .split_once("-- FREIGHT_CANONICAL_BACKFILL")
            .expect("boundary");
        let mut tx = self.pool.begin().await.expect("historical transaction");
        sqlx::raw_sql(setup)
            .execute(&mut *tx)
            .await
            .expect("historical setup");
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT tenant_id,data::text FROM freight_workspaces ORDER BY tenant_id",
        )
        .fetch_all(&mut *tx)
        .await
        .expect("historical originals");
        for (tenant, raw) in rows {
            let mut frame = stateset_freight_storage_codec::pack(
                &raw,
                stateset_freight_storage_codec::Config {
                    span: 64,
                    target_bytes: 65536,
                },
            )
            .expect("historical pack");
            frame
                .roots
                .get_mut("_freight_format")
                .expect("format")
                .scalar = RawValue::from_string("19".into()).expect("marker");
            for block in std::mem::take(&mut frame.blocks) {
                sqlx::query("INSERT INTO freight_workspace_blocks VALUES($1,$2,$3,$4)")
                    .bind(&tenant)
                    .bind(&block.root)
                    .bind(&block.key)
                    .bind(sqlx::types::Json(
                        serde_json::value::to_raw_value(&block).expect("raw block"),
                    ))
                    .execute(&mut *tx)
                    .await
                    .expect("original block");
            }
            #[derive(serde::Serialize)]
            struct Header {
                _freight_format: i32,
                frame: stateset_freight_storage_codec::Packed,
            }
            let header = serde_json::value::to_raw_value(&Header {
                _freight_format: 19,
                frame,
            })
            .expect("header");
            sqlx::query("UPDATE freight_workspaces SET data=$2 WHERE tenant_id=$1")
                .bind(&tenant)
                .bind(sqlx::types::Json(header))
                .execute(&mut *tx)
                .await
                .expect("historical header");
        }
        sqlx::raw_sql(finish)
            .execute(&mut *tx)
            .await
            .expect("historical finish");
        sqlx::query("INSERT INTO freight_schema_migrations(version,checksum) VALUES(26,$1)")
            .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
            .execute(&mut *tx)
            .await
            .expect("historical checksum");
        tx.commit().await.expect("historical commit");
    }
    async fn tuples(&self, tenant: &str) -> BTreeMap<(String, String), (String, String, String)> {
        let rows:Vec<(String,String,String,String,String)>=sqlx::query_as("SELECT root,key,xmin::text,ctid::text,payload::text FROM freight_workspace_blocks WHERE tenant_id=$1 ORDER BY root,key")
            .bind(tenant).fetch_all(&self.pool).await.expect("physical source rows");
        rows.into_iter()
            .map(|(r, k, x, c, p)| ((r, k), (x, c, p)))
            .collect()
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
async fn postgres_policy_fence_preserves_exact_blocks_files_metadata_and_historical_receipts() {
    let db = Database::new().await;
    db.legacy().await;
    db.put_legacy("demo", original("18", true)).await;
    db.canonical19().await;
    sqlx::query("INSERT INTO freight_document_content VALUES('demo','policy-original',$1)")
        .bind(b"Original policy evidence\0\xff".as_slice())
        .execute(&db.pool)
        .await
        .expect("original file");
    let before = db.tuples("demo").await;
    let header: String =
        sqlx::query_scalar("SELECT data::text FROM freight_workspaces WHERE tenant_id='demo'")
            .fetch_one(&db.pool)
            .await
            .expect("original header");
    let metadata: (String, i64, Option<String>, Option<String>, Option<String>) = sqlx::query_as("SELECT updated_at::text,history_revision,inbox_due_at::text,edi_due_at::text,sync_due_at::text FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("metadata");
    let originals: String = sqlx::query_scalar("SELECT freight_workspace_originals('demo')::text")
        .fetch_one(&db.pool)
        .await
        .expect("complete originals");
    let mut preceding = db
        .pool
        .acquire()
        .await
        .expect("already open preceding connection");
    sqlx::query("PREPARE preceding_policy_writer(jsonb) AS UPDATE freight_workspaces SET data=$1 WHERE tenant_id='demo'").execute(&mut *preceding).await.expect("preceding statement prepared before upgrade");
    let (a, b) = tokio::join!(Store::new(Some(&db.url)), Store::new(Some(&db.url)));
    let a = a.expect("upgrade");
    let b = b.expect("concurrent upgrade");
    assert_eq!(
        db.tuples("demo").await,
        before,
        "Every original block retains payload, xmin and ctid"
    );
    let after: String = sqlx::query_scalar("SELECT jsonb_set(jsonb_set(data,'{_freight_format}','19'::jsonb),'{frame,roots,_freight_format,scalar}','19'::jsonb)::text FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("metadata-only marker change");
    assert_eq!(after, header);
    let after_metadata: (String, i64, Option<String>, Option<String>, Option<String>) = sqlx::query_as("SELECT updated_at::text,history_revision,inbox_due_at::text,edi_due_at::text,sync_due_at::text FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("unchanged scheduling");
    assert_eq!(after_metadata, metadata);
    let after_originals: String = sqlx::query_scalar("SELECT jsonb_set(freight_workspace_originals('demo'),'{_freight_format}','19'::jsonb)::text").fetch_one(&db.pool).await.expect("exact originals");
    assert_eq!(
        after_originals, originals,
        "Opaque decimals, literal keys and original receipt bodies remain exact"
    );
    let file: Vec<u8> = sqlx::query_scalar("SELECT content FROM freight_document_content WHERE tenant_id='demo' AND document_id='policy-original'").fetch_one(&db.pool).await.expect("file retained");
    assert_eq!(file, b"Original policy evidence\0\xff");
    let execute = format!(
        "EXECUTE preceding_policy_writer('{}'::jsonb)",
        header.replace('\'', "''")
    );
    let error = sqlx::raw_sql(&execute)
        .execute(&mut *preceding)
        .await
        .expect_err("already prepared preceding writer refused");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    assert_eq!(db.tuples("demo").await, before);
    assert_eq!(
        a.read("demo").await.expect("native source").loads.len(),
        b.read("demo").await.expect("second source").loads.len()
    );
    drop(preceding);
    drop(a);
    drop(b);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_policy_fence_refuses_corrupt_frame_without_changing_registry_triggers_or_ledger()
{
    let db = Database::new().await;
    db.legacy().await;
    db.put_legacy("a-valid", original("18", true)).await;
    db.put_legacy("z-damaged", original("18", false)).await;
    db.canonical19().await;
    let (root,key,payload):(String,String,serde_json::Value)=sqlx::query_as("SELECT root,key,payload FROM freight_workspace_blocks WHERE tenant_id='z-damaged' AND root='loads' ORDER BY key LIMIT 1").fetch_one(&db.pool).await.expect("real block");
    sqlx::query("UPDATE freight_workspace_blocks SET payload=jsonb_set(payload,'{items,0,position}','0') WHERE tenant_id='z-damaged' AND root=$1 AND key=$2").bind(&root).bind(&key).execute(&db.pool).await.expect("bad ordinal fixture");
    let original_rows:Vec<(String,String,String,String)>=sqlx::query_as("SELECT tenant_id,data::text,xmin::text,ctid::text FROM freight_workspaces ORDER BY tenant_id").fetch_all(&db.pool).await.expect("before");
    let blocks = db.tuples("a-valid").await;
    let damaged = db.tuples("z-damaged").await;
    assert!(
        Store::new(Some(&db.url)).await.is_err(),
        "Full native validation must reject zero array ordinal, even when counts match"
    );
    let after:Vec<(String,String,String,String)>=sqlx::query_as("SELECT tenant_id,data::text,xmin::text,ctid::text FROM freight_workspaces ORDER BY tenant_id").fetch_all(&db.pool).await.expect("after");
    assert_eq!(after, original_rows);
    assert_eq!(db.tuples("a-valid").await, blocks);
    assert_eq!(db.tuples("z-damaged").await, damaged);
    let (format,count):(i32,i64)=sqlx::query_as("SELECT version,(SELECT count(*) FROM freight_schema_migrations) FROM freight_storage_format WHERE singleton").fetch_one(&db.pool).await.expect("registry");
    assert_eq!((format, count), (19, 26));
    let enabled:bool=sqlx::query_scalar("SELECT bool_and(tgenabled='O') FROM pg_trigger WHERE tgrelid='freight_workspaces'::regclass AND NOT tgisinternal").fetch_one(&db.pool).await.expect("guards");
    assert!(enabled);
    sqlx::query("UPDATE freight_workspace_blocks SET payload=$3 WHERE tenant_id='z-damaged' AND root=$1 AND key=$2").bind(&root).bind(&key).bind(payload).execute(&db.pool).await.expect("exact source restored");
    Store::new(Some(&db.url))
        .await
        .expect("Retry only after exact repair");
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_policy_fence_waits_for_inflight_preceding_writer_and_retains_its_committed_original(
) {
    let db = Database::new().await;
    db.legacy().await;
    db.put_legacy("demo", original("18", false)).await;
    db.canonical19().await;
    let mut old = db.pool.begin().await.expect("inflight preceding writer");
    sqlx::query(
        "SELECT version FROM freight_storage_format WHERE singleton AND version=19 FOR SHARE",
    )
    .fetch_one(&mut *old)
    .await
    .expect("old release format lock");
    let name = format!("policy-migration-{}", uuid::Uuid::new_v4().simple());
    let mut url = reqwest::Url::parse(&db.url).expect("url");
    url.query_pairs_mut().append_pair("application_name", &name);
    let (upgraded, ()) = tokio::join!(Store::new(Some(url.as_str())), async {
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND query LIKE '%version=19 FOR UPDATE%')").bind(&name).fetch_one(&db.pool).await.expect("observe actual format lock wait");
            if waiting {break;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("Migration waits for preceding writer");
        sqlx::query("UPDATE freight_workspace_blocks SET payload=jsonb_set(payload,'{items,0,payload,location}','\"Committed before policy fence\"') WHERE tenant_id='demo' AND root='loads' AND key=(SELECT min(key) FROM freight_workspace_blocks WHERE tenant_id='demo' AND root='loads')").execute(&mut *old).await.expect("preceding physical command");
        // Old canonical writers publish their corresponding header last.
        sqlx::query("UPDATE freight_workspaces SET data=data WHERE tenant_id='demo'")
            .execute(&mut *old)
            .await
            .expect("preceding header");
        old.commit()
            .await
            .expect("old transaction finishes before fence");
    });
    let store = upgraded.expect("migration after preceding commit");
    assert_eq!(
        store.read("demo").await.expect("current reader").loads[0].location,
        "Committed before policy fence"
    );
    let version: i32 =
        sqlx::query_scalar("SELECT version FROM freight_storage_format WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("fenced version");
    assert_eq!(version, stateset_freight::store::STORAGE_FORMAT);
    drop(store);
    db.clean().await;
}
fn original(marker: &str, opaque: bool) -> Box<RawValue> {
    let mut roots: BTreeMap<String, Box<RawValue>> =
        serde_json::from_str(&serde_json::to_string(&domain::demo()).expect("demo"))
            .expect("roots");
    roots.insert(
        "_freight_format".into(),
        RawValue::from_string(marker.into()).expect("marker"),
    );
    if opaque {
        roots.insert("operations_cases".into(),RawValue::from_string(r#"[{"future_proof":{"n":18446744073709551616001,"d":1.00000000000000000001,"$serde_json::private::Number":"literal"}}]"#.into()).expect("opaque cases"));
        roots.insert("operations_policy".into(),RawValue::from_string(r#"{"version":99,"tracking_stale_minutes":240,"future":{"decimal":123.45000000000000000001,"literal":"雪"}}"#.into()).expect("opaque policy"));
        roots.insert("receipts".into(),RawValue::from_string(r#"{"literal/\"雪":{"fingerprint":"retained","response":{"exact":1.00000000000000000001,"large":18446744073709551616001}}}"#.into()).expect("original receipt"));
    }
    serde_json::value::to_raw_value(&roots).expect("original raw")
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_canonical_legacy_optional_collections_remain_empty_without_hiding_missing_originals(
) {
    let db = Database::new().await;
    let store = Store::new(Some(&db.url)).await.expect("current store");
    let mut legacy = json!(domain::demo());
    legacy["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    for root in ["appointments", "stop_events", "inbox"] {
        legacy.as_object_mut().expect("object").remove(root);
    }
    canonical::put(&db.pool, "demo", &legacy)
        .await
        .expect("legacy optional fields absent");
    assert!(store
        .read("demo")
        .await
        .expect("native legacy read")
        .appointments
        .is_empty());
    for kind in ["appointments", "gates"] {
        let query = json!({"kind":kind,"load_id":"load_1","limit":2});
        let page = store
            .history_page("demo", serde_json::from_value(query).expect("query"))
            .await
            .expect("absent optional history is empty");
        assert_eq!(page["items"], json!([]));
        assert_eq!(page["total"], 0);
    }
    let due: Option<String> = sqlx::query_scalar(
        "SELECT inbox_due_at::text FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("legacy inbox hint");
    assert_eq!(due, None, "absent default inbox must remain quiet");
    store
        .mutate(
            "demo",
            "legacy-location",
            "load.command",
            "load_1",
            json!({"action":"location","expected_version":1,"location":"Actual legacy location"}),
        )
        .await
        .expect("unrelated native mutation");
    let retained = canonical::source(&db.pool, "demo")
        .await
        .expect("originals after mutation");
    for root in ["appointments", "stop_events", "inbox"] {
        assert!(
            !retained.as_object().expect("object").contains_key(root),
            "unchanged default must not materialize {root}"
        );
    }
    for (root, kind) in [("appointments", "appointments"), ("stop_events", "gates")] {
        let mut corrupt = legacy.clone();
        corrupt[root] = serde_json::Value::Null;
        canonical::put(&db.pool, "demo", &corrupt)
            .await
            .expect("explicit damaged collection");
        let before = canonical::physical(&db.pool, "demo")
            .await
            .expect("damaged originals");
        assert!(store.read("demo").await.is_err());
        assert!(
            store
                .history_page(
                    "demo",
                    serde_json::from_value(json!({"kind":kind,"load_id":"load_1"})).expect("query")
                )
                .await
                .is_err(),
            "explicit null is not an absent optional collection"
        );
        assert_eq!(
            canonical::physical(&db.pool, "demo")
                .await
                .expect("retained damage"),
            before
        );
    }
    canonical::put(&db.pool, "demo", &legacy)
        .await
        .expect("restore original source");
    let removed = sqlx::query(
        "DELETE FROM freight_workspace_blocks WHERE tenant_id='demo' AND root='events'",
    )
    .execute(&db.pool)
    .await
    .expect("remove declared retained originals");
    assert!(
        removed.rows_affected() > 0,
        "the corruption must remove actual originals"
    );
    assert!(store.read("demo").await.is_err());
    assert!(
        store
            .history_page(
                "demo",
                serde_json::from_value(json!({"kind":"events"})).expect("query")
            )
            .await
            .is_err(),
        "declared originals cannot fall back to empty"
    );
    drop(store);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_canonical_backfill_retains_every_original_and_native_deltas_leave_unrelated_rows_untouched(
) {
    let db = Database::new().await;
    db.legacy().await;
    db.put_legacy("demo", original("18", true)).await;
    let mut extended: BTreeMap<String, Box<RawValue>> =
        serde_json::from_str(original("18", false).get()).expect("roots");
    extended.insert(
        "future_financial_obligation".into(),
        RawValue::from_string(r#"{"amount":123.00000000000000000001}"#.into())
            .expect("unknown root"),
    );
    db.put_legacy(
        "extended",
        serde_json::value::to_raw_value(&extended).expect("extended"),
    )
    .await;
    let expected:Vec<(String,String)>=sqlx::query_as("SELECT tenant_id,jsonb_set(data,'{_freight_format}','21'::jsonb)::text FROM freight_workspaces ORDER BY tenant_id")
        .fetch_all(&db.pool).await.expect("complete original business source");
    let revisions:Vec<(String,i64,String)>=sqlx::query_as("SELECT tenant_id,history_revision,updated_at::text FROM freight_workspaces ORDER BY tenant_id").fetch_all(&db.pool).await.expect("portable metadata");
    sqlx::query("INSERT INTO freight_document_content VALUES('demo','original',$1)")
        .bind(b"original file bytes\0\xff".as_slice())
        .execute(&db.pool)
        .await
        .expect("file");
    let (a, b) = tokio::join!(Store::new(Some(&db.url)), Store::new(Some(&db.url)));
    let store = a.expect("native migration");
    let other = b.expect("concurrent startup");
    let actual:Vec<(String,String)>=sqlx::query_as("SELECT tenant_id,freight_workspace_originals(tenant_id)::text FROM freight_workspaces ORDER BY tenant_id").fetch_all(&db.pool).await.expect("independent SQL reconstruction");
    assert_eq!(
        actual, expected,
        "all original JSONB values and decimal scale must survive"
    );
    let after:Vec<(String,i64,String)>=sqlx::query_as("SELECT tenant_id,history_revision,updated_at::text FROM freight_workspaces ORDER BY tenant_id").fetch_all(&db.pool).await.expect("metadata");
    assert_eq!(after, revisions);
    let inline:bool=sqlx::query_scalar("SELECT bool_and(NOT data ? 'loads' AND data->'frame'->'blocks'='[]'::jsonb) FROM freight_workspaces").fetch_one(&db.pool).await.expect("canonical header");
    assert!(inline);
    let bytes:Vec<u8>=sqlx::query_scalar("SELECT content FROM freight_document_content WHERE tenant_id='demo' AND document_id='original'").fetch_one(&db.pool).await.expect("original bytes");
    assert_eq!(bytes, b"original file bytes\0\xff");
    assert!(
        store.read("extended").await.is_err(),
        "unknown business roots remain unreadable, not dropped"
    );
    let mut legacy = original("18", false);
    let error = sqlx::query("UPDATE freight_workspaces SET data=$1 WHERE tenant_id='demo'")
        .bind(sqlx::types::Json(&legacy))
        .execute(&db.pool)
        .await
        .expect_err("obsolete writer refused");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    let before = db.tuples("demo").await;
    let isolated = db.tuples("extended").await;
    let opaque:(String,String,String)=sqlx::query_as("SELECT (freight_workspace_originals('demo')->'operations_cases')::text,(freight_workspace_originals('demo')->'operations_policy')::text,(freight_workspace_originals('demo')->'receipts'->'literal/\"雪')::text")
        .fetch_one(&db.pool).await.expect("exact opaque originals");
    let response=store.mutate("demo","new-location","load.command","load_1",json!({"action":"location","expected_version":1,"location":"Actual canonical position"})).await.expect("native canonical mutation");
    assert_eq!(response["data"]["location"], "Actual canonical position");
    let after = db.tuples("demo").await;
    let mut preserved = 0;
    let mut changed = 0;
    for (key, old) in &before {
        if let Some(new) = after.get(key) {
            if old.2 == new.2 {
                assert_eq!(old, new, "unchanged payload must retain its physical tuple");
                preserved += 1;
            } else {
                changed += 1;
            }
        }
    }
    assert!(preserved > 5);
    assert!(changed <= 4, "only actual record blocks may be rewritten");
    assert_eq!(db.tuples("extended").await, isolated);
    let retained:(String,String,String)=sqlx::query_as("SELECT (freight_workspace_originals('demo')->'operations_cases')::text,(freight_workspace_originals('demo')->'operations_policy')::text,(freight_workspace_originals('demo')->'receipts'->'literal/\"雪')::text")
        .fetch_one(&db.pool).await.expect("retained exact original fields");
    assert_eq!(retained, opaque);
    let physical = db.tuples("demo").await;
    let replay=other.mutate("demo","new-location","load.command","load_1",json!({"action":"location","expected_version":1,"location":"Actual canonical position"})).await.expect("original retry");
    assert_eq!(replay["data"], response["data"]);
    assert_eq!(db.tuples("demo").await, physical);
    assert_eq!(
        other.read("demo").await.expect("reconnect source").loads[0].location,
        "Actual canonical position"
    );
    // Keep both lookup and counter fences tied to actual stored rows.
    let count:bool=sqlx::query_scalar("SELECT bool_and(freight_canonical_root_valid('demo',data,name)) FROM freight_workspaces CROSS JOIN LATERAL jsonb_object_keys(data->'frame'->'roots') name WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("all root memberships");
    assert!(count);
    legacy = original("21", false);
    let error = sqlx::query("UPDATE freight_workspaces SET data=$1 WHERE tenant_id='demo'")
        .bind(sqlx::types::Json(legacy))
        .execute(&db.pool)
        .await
        .expect_err("aggregate writer with current marker refused");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    drop(store);
    drop(other);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_canonical_backfill_refusal_rolls_back_earlier_tenants_ddl_ledger_and_original_files(
) {
    let db = Database::new().await;
    db.legacy().await;
    db.put_legacy("a-original", original("18", true)).await;
    // The preceding SQL guard accepts JSONB numeric equality; native format
    // compatibility requires the exact integral marker, so this must refuse.
    db.put_legacy("z-unsupported", original("18.0", false))
        .await;
    sqlx::query("INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES('a-original','retained',$1)")
        .bind(b"retained through failed migration\0\xff".as_slice()).execute(&db.pool).await.expect("original file");
    let before:Vec<(String,String,String,String)>=sqlx::query_as("SELECT tenant_id,data::text,xmin::text,ctid::text FROM freight_workspaces ORDER BY tenant_id").fetch_all(&db.pool).await.expect("before");
    assert!(
        Store::new(Some(&db.url)).await.is_err(),
        "incompatible source must refuse migration"
    );
    let after:Vec<(String,String,String,String)>=sqlx::query_as("SELECT tenant_id,data::text,xmin::text,ctid::text FROM freight_workspaces ORDER BY tenant_id").fetch_all(&db.pool).await.expect("after");
    assert_eq!(after, before);
    let tables:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.tables WHERE table_schema=current_schema() AND table_name IN ('freight_workspace_blocks','freight_block_counts')").fetch_one(&db.pool).await.expect("DDL rollback");
    assert_eq!(tables, 0);
    let (version,migrations):(i32,i64)=sqlx::query_as("SELECT version,(SELECT count(*) FROM freight_schema_migrations) FROM freight_storage_format WHERE singleton").fetch_one(&db.pool).await.expect("original history");
    assert_eq!((version, migrations), (18, 25));
    let file:Vec<u8>=sqlx::query_scalar("SELECT content FROM freight_document_content WHERE tenant_id='a-original' AND document_id='retained'")
        .fetch_one(&db.pool).await.expect("original file after rollback");
    assert_eq!(file, b"retained through failed migration\0\xff");
    db.clean().await;
}
