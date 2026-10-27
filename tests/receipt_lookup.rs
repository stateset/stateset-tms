use serde_json::{json, Value};
use sqlx::PgPool;
use stateset_freight::{
    save_recovery::REQUEST_STOPPED,
    store::{Store, INCOMPATIBLE_STORAGE},
};
#[path = "support/canonical.rs"]
mod canonical;

struct Database {
    admin: PgPool,
    pool: PgPool,
    schema: String,
    url: String,
}
impl Database {
    async fn new() -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("receipt_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("url");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema},pg_catalog"))
            .append_pair("application_name", &schema);
        let pool = PgPool::connect(url.as_str()).await.expect("pool");
        Self {
            admin,
            pool,
            schema,
            url: url.to_string(),
        }
    }
    async fn raw(&self) -> Value {
        canonical::source(&self.pool, "demo")
            .await
            .expect("original source")
    }
    async fn physical(&self) -> Value {
        canonical::physical(&self.pool, "demo")
            .await
            .expect("physical originals")
    }
    async fn put(&self, value: &Value) {
        canonical::put(&self.pool, "demo", value)
            .await
            .expect("owned source");
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
fn body() -> Value {
    json!({"action":"location","expected_version":1,"location":"Original observed position"})
}
const KEY: &str = "original/\"{key}\\雪.0";
async fn replay(store: &Store, tenant: &str, key: &str) -> Result<Option<Value>, String> {
    store
        .replay(tenant, key, "load.command", "load_1", &body())
        .await
}
async fn save(store: &Store, key: &str) -> Result<Value, String> {
    store
        .mutate_authorized(("demo", "operator"), key, "load.command", "load_1", body())
        .await
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_selected_receipt_keeps_exact_original_identity_stop_and_format_guards() {
    let db = Database::new().await;
    let store = Store::new(Some(&db.url)).await.expect("API");
    store.seed_demo().await.expect("seed");
    assert_eq!(replay(&store, "absent", KEY).await.expect("absent"), None);
    let original = save(&store, KEY).await.expect("original application");
    let expected = json!({"data":original["data"],"replayed":true});
    let source = db.raw().await;
    let before = db.physical().await;
    assert_eq!(
        replay(&store, "demo", KEY).await.expect("selected"),
        Some(expected.clone())
    );
    assert_eq!(
        save(&store, KEY).await.expect("ordinary command retry"),
        expected
    );
    assert_eq!(replay(&store, "demo", "missing").await.expect("miss"), None);
    assert_eq!(
        replay(&store, "other", KEY)
            .await
            .expect("tenant isolation"),
        None
    );
    assert_eq!(
        db.physical().await,
        before,
        "lookup/retry cannot rewrite the source"
    );
    for (operation, entity, changed) in [
        ("load.command", "load_2", body()),
        ("load.revise", "load_1", body()),
        (
            "load.command",
            "load_1",
            json!({"action":"location","expected_version":1,"location":"Changed request"}),
        ),
    ] {
        assert_eq!(
            store
                .replay("demo", KEY, operation, entity, &changed)
                .await
                .expect_err("same key, different request"),
            "Idempotency key already used for another request"
        );
    }
    let mut stopped = source.clone();
    stopped["receipts"][KEY]["stopped"] = json!({
        "path":"loads/load_1/commands","note":"Stop this original save",
        "stopped_by":"operator","stopped_at":"2026-10-17T00:00:00Z"
    });
    db.put(&stopped).await;
    let before = db.physical().await;
    assert_eq!(
        replay(&store, "demo", KEY).await.expect_err("stopped"),
        REQUEST_STOPPED
    );
    assert_eq!(
        save(&store, KEY).await.expect_err("cannot revive"),
        REQUEST_STOPPED
    );
    assert_eq!(db.physical().await, before);

    for malformed in [
        json!(null),
        json!([]),
        json!({"fingerprint":"missing response"}),
    ] {
        let mut damaged = source.clone();
        damaged["receipts"][KEY] = malformed;
        db.put(&damaged).await;
        assert!(replay(&store, "demo", KEY).await.is_err());
        assert!(save(&store, KEY).await.is_err());
    }
    for malformed in [json!(null), json!([]), json!("bad collection"), json!([{}])] {
        let mut damaged = source.clone();
        damaged["receipts"] = malformed;
        db.put(&damaged).await;
        assert!(replay(&store, "demo", KEY).await.is_err());
        assert!(replay(&store, "demo", "missing").await.is_err());
    }
    let mut missing = source.clone();
    missing.as_object_mut().expect("object").remove("receipts");
    db.put(&missing).await;
    assert!(replay(&store, "demo", KEY).await.is_err());

    db.put(&source).await;
    canonical::corrupt_marker(&db.pool, "demo", "19.0")
        .await
        .expect("owned decimal marker damage");
    assert_eq!(
        replay(&store, "demo", KEY).await.expect_err("exact marker"),
        INCOMPATIBLE_STORAGE
    );
    assert_eq!(
        replay(&store, "demo", "missing")
            .await
            .expect_err("format before miss"),
        INCOMPATIBLE_STORAGE
    );
    db.put(&source).await;
    sqlx::query("UPDATE freight_storage_format SET version=22 WHERE singleton")
        .execute(&db.pool)
        .await
        .expect("owned upgrade");
    for tenant in ["demo", "absent"] {
        assert_eq!(
            replay(&store, tenant, KEY)
                .await
                .expect_err("registry before lookup"),
            INCOMPATIBLE_STORAGE
        );
    }
    sqlx::query("UPDATE freight_storage_format SET version=21 WHERE singleton")
        .execute(&db.pool)
        .await
        .expect("restore registry");
    drop(store);
    let reopened = Store::new(Some(&db.url)).await.expect("reconnect");
    assert_eq!(
        replay(&reopened, "demo", KEY)
            .await
            .expect("durable original"),
        Some(expected)
    );
    drop(reopened);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_receipt_slice_skips_unrelated_history_but_never_lets_new_saves_write_through_damage(
) {
    let db = Database::new().await;
    let store = Store::new(Some(&db.url)).await.expect("API");
    store.seed_demo().await.expect("seed");
    let original = save(&store, KEY).await.expect("original");
    let expected = json!({"data":original["data"],"replayed":true});
    let mut source = db.raw().await;
    // Large, valid retained receipts must not be sent/decoded just to return one
    // immutable original. Use varied bytes rather than a compressible padding.
    let mut retained = source["receipts"][KEY].clone();
    for i in 0..1500 {
        retained["response"] = json!({"retained":(0..64).map(|j| format!("{}", uuid::Uuid::new_v4().as_u128() ^ (i * 64 + j))).collect::<Vec<_>>()});
        source["receipts"][format!("retained-{i}")] = retained.clone();
    }
    db.put(&source).await;
    assert!(store.read("demo").await.is_ok(), "valid retained models");
    let (whole_bytes, selected_bytes): (i32, i32) = sqlx::query_as("SELECT octet_length(freight_workspace_originals(tenant_id)::text),octet_length(freight_canonical_receipt(tenant_id,data,$1)::text) FROM freight_workspaces WHERE tenant_id='demo'")
        .bind(KEY).fetch_one(&db.pool).await.expect("actual wire sizes");
    assert!(whole_bytes > 3_000_000);
    assert!(selected_bytes < 8192);
    let before = db.physical().await;
    assert_eq!(
        save(&store, KEY).await.expect("selected original"),
        expected
    );
    assert_eq!(db.physical().await, before);

    // This read certifies the chosen receipt, not the rest of the workspace.
    // A new command must still use the locked, strict full-source check.
    source["events"] = json!("malformed unrelated retained history");
    source["receipts"]["unrelated-damaged-receipt"] = json!(null);
    db.put(&source).await;
    let before = db.physical().await;
    assert!(store.read("demo").await.is_err());
    assert_eq!(
        replay(&store, "demo", KEY)
            .await
            .expect("original remains readable"),
        Some(expected.clone())
    );
    assert_eq!(
        save(&store, KEY)
            .await
            .expect("original remains replayable"),
        expected
    );
    let mut current = body();
    current["expected_version"] = json!(2);
    assert_eq!(
        store
            .mutate_authorized(
                ("demo", "operator"),
                "new-key",
                "load.command",
                "load_1",
                current,
            )
            .await
            .expect_err("a valid new command cannot bypass strict source validation"),
        "Internal storage error",
        "refusal must be source integrity, not a stale-version conflict"
    );
    assert_eq!(db.physical().await, before);

    if let Ok(path) = std::env::var("FREIGHT_RECEIPT_SLICE_EVIDENCE") {
        let plan: Value = sqlx::query_scalar(&format!(
            "EXPLAIN (ANALYZE,BUFFERS,VERBOSE,FORMAT JSON) {}",
            include_str!("../src/receipt_lookup.sql")
        ))
        .bind("demo")
        .bind(KEY)
        .bind(stateset_freight::store::STORAGE_FORMAT.to_string())
        .fetch_one(&db.pool)
        .await
        .expect("selected lookup plan");
        std::fs::write(path, serde_json::to_vec_pretty(&json!({
            "result":"pass","retained_receipts":1500,
            "source_text_bytes":whole_bytes,"selected_receipt_text_bytes":selected_bytes,
            "source_unchanged_by_replays":true,"plan":plan,
            "scope":"Synthetic PostgreSQL lookup sizes; JSONB decompression still occurs. No general memory or latency bound."
        })).expect("JSON")).expect("evidence");
    }
    drop(store);
    db.clean().await;
}
