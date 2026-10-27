#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request};
use serde_json::{json, Value};
use sqlx::PgPool;
use stateset_freight::{
    api::{self, App},
    domain,
    integrations::Integrations,
    store::{Store, INCOMPATIBLE_STORAGE},
};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

const PREVIOUS_MIGRATIONS: [&str; 25] = [
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

// Every test owns a separate PostgreSQL schema, including its format registry.
// Changing the simulated release version cannot interfere with other suites.
struct Database {
    admin: PgPool,
    pool: PgPool,
    url: String,
    schema: String,
}
impl Database {
    async fn new() -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("disposable TEST_DATABASE_URL");
        let admin = PgPool::connect(&base).await.expect("admin connection");
        let schema = format!("freight_test_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("isolated schema");
        let mut url = reqwest::Url::parse(&base).expect("database URL");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let pool = PgPool::connect(url.as_str()).await.expect("isolated pool");
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
            .expect("drop owned schema");
        self.admin.close().await;
    }
    async fn raw(&self, tenant: &str) -> Value {
        let canonical_storage: bool =
            sqlx::query_scalar("SELECT to_regclass('freight_workspace_blocks') IS NOT NULL")
                .fetch_one(&self.pool)
                .await
                .expect("fixture representation");
        if canonical_storage {
            return canonical::source(&self.pool, tenant)
                .await
                .expect("canonical originals");
        }
        sqlx::query_scalar("SELECT data FROM freight_workspaces WHERE tenant_id=$1")
            .bind(tenant)
            .fetch_one(&self.pool)
            .await
            .expect("raw persisted data")
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_upgrade_preserves_legacy_data_files_and_rejects_obsolete_writers() {
    let db = Database::new().await;
    sqlx::raw_sql(include_str!("../migrations/001_workspaces.sql"))
        .execute(&db.pool)
        .await
        .expect("legacy aggregate table");
    sqlx::raw_sql(include_str!("../migrations/002_documents.sql"))
        .execute(&db.pool)
        .await
        .expect("legacy content table");
    let legacy = json!(domain::demo());
    let mut extended = legacy.clone();
    extended["future_finance_data"] = json!({"amount_cents":9876543,"evidence":"retain exactly"});
    for (tenant, data) in [("legacy", &legacy), ("extended", &extended)] {
        sqlx::query("INSERT INTO freight_workspaces(tenant_id,data,updated_at) VALUES($1,$2,'2026-01-02T03:04:05Z')")
            .bind(tenant).bind(data).execute(&db.pool).await.expect("legacy data");
    }
    let bytes = b"%PDF-1.7\nlegacy original bytes\n%%EOF";
    sqlx::query("INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES('legacy','original',$1)")
        .bind(bytes.as_slice()).execute(&db.pool).await.expect("legacy file");

    let (a, b) = tokio::join!(Store::new(Some(&db.url)), Store::new(Some(&db.url)));
    let a = a.expect("first concurrent upgrade");
    let b = b.expect("second concurrent upgrade");
    for (tenant, original) in [("legacy", &legacy), ("extended", &extended)] {
        let mut upgraded = db.raw(tenant).await;
        assert_eq!(
            upgraded
                .as_object_mut()
                .expect("object")
                .remove("_freight_format"),
            Some(json!(stateset_freight::store::STORAGE_FORMAT))
        );
        assert_eq!(
            &upgraded, original,
            "migration may only add the format marker"
        );
    }
    let timestamp: String = sqlx::query_scalar("SELECT (updated_at AT TIME ZONE 'UTC')::text FROM freight_workspaces WHERE tenant_id='legacy'")
        .fetch_one(&db.pool).await.expect("timestamp");
    assert_eq!(timestamp, "2026-01-02 03:04:05");
    let preserved: Vec<u8> = sqlx::query_scalar("SELECT content FROM freight_document_content WHERE tenant_id='legacy' AND document_id='original'")
        .fetch_one(&db.pool).await.expect("file");
    assert_eq!(preserved, bytes);
    assert_eq!(json!(a.read("legacy").await.expect("legacy read")), legacy);
    // An unrecognized root collection cannot silently disappear on a rewrite.
    assert!(a.read("extended").await.is_err());
    assert!(a
        .mutate(
            "extended",
            "unsafe",
            "customer.create",
            "",
            json!({"name":"Must not write","contact":""})
        )
        .await
        .is_err());
    let before = db.raw("legacy").await;
    for stale in [
        legacy.clone(),
        json!({"_freight_format":0}),
        json!({"_freight_format":"1"}),
        json!(null),
        json!([]),
    ] {
        let error = sqlx::query("UPDATE freight_workspaces SET data=$1 WHERE tenant_id='legacy'")
            .bind(stale)
            .execute(&db.pool)
            .await
            .expect_err("obsolete writer must fail");
        assert_eq!(
            error.as_database_error().and_then(|v| v.code()).as_deref(),
            Some("23514")
        );
        assert_eq!(db.raw("legacy").await, before);
    }
    assert!(
        sqlx::query("INSERT INTO freight_workspaces(tenant_id,data) VALUES('old-insert',$1)")
            .bind(&legacy)
            .execute(&db.pool)
            .await
            .is_err()
    );
    // A previous API can insert file bytes before rewriting its aggregate.
    // The failed rewrite must abort that entire transaction, not leave a file.
    let mut stale_writer = db.pool.begin().await.expect("old writer transaction");
    sqlx::query("INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES('legacy','obsolete-upload',$1)")
        .bind(bytes.as_slice()).execute(&mut *stale_writer).await.expect("uncommitted old upload");
    assert!(
        sqlx::query("UPDATE freight_workspaces SET data=$1 WHERE tenant_id='legacy'")
            .bind(&legacy)
            .execute(&mut *stale_writer)
            .await
            .is_err()
    );
    stale_writer
        .rollback()
        .await
        .expect("old transaction rollback");
    let orphan_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM freight_document_content WHERE document_id='obsolete-upload'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("orphan check");
    assert_eq!(orphan_count, 0);
    assert_eq!(db.raw("legacy").await, before);
    a.mutate(
        "legacy",
        "supported",
        "customer.create",
        "",
        json!({"name":"Supported writer","contact":""}),
    )
    .await
    .expect("current writer");
    assert_eq!(
        b.read("legacy").await.expect("read").customers.len(),
        before["customers"].as_array().expect("customers").len() + 1
    );
    drop((a, b));
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_newer_release_fences_running_api_reads_mutations_replay_and_startup() {
    let db = Database::new().await;
    let a = Store::new(Some(&db.url)).await.expect("start");
    let body = json!({"name":"Existing customer","contact":""});
    a.mutate("tenant", "original", "customer.create", "", body.clone())
        .await
        .expect("persist");
    let mut upgraded = db.raw("tenant").await;
    upgraded["future_finance_data"] = json!([{"amount_cents":7654321}]);
    canonical::put(&db.pool, "tenant", &upgraded)
        .await
        .expect("retained future data");
    assert_eq!(db.raw("tenant").await, upgraded);
    let mut tx = db.pool.begin().await.expect("upgrade transaction");
    sqlx::query("UPDATE freight_storage_format SET version=22 WHERE singleton")
        .execute(&mut *tx)
        .await
        .expect("new release");
    canonical::corrupt_marker_tx(&mut tx, "tenant", "22")
        .await
        .expect("simulate future header marker");
    tx.commit().await.expect("upgrade commit");
    let future_originals = canonical::physical(&db.pool, "tenant")
        .await
        .expect("future originals");
    assert!(!a.healthy().await);
    assert_eq!(
        a.read("tenant").await.expect_err("read fenced"),
        INCOMPATIBLE_STORAGE
    );
    assert_eq!(
        a.read("empty").await.expect_err("empty tenant fenced"),
        INCOMPATIBLE_STORAGE
    );
    assert_eq!(
        a.tenant_ids().await.expect_err("worker enumeration fenced"),
        INCOMPATIBLE_STORAGE
    );
    assert_eq!(
        a.seed_demo().await.expect_err("seed fenced"),
        INCOMPATIBLE_STORAGE
    );
    assert_eq!(
        a.replay("tenant", "original", "customer.create", "", &body)
            .await
            .expect_err("replay fenced"),
        INCOMPATIBLE_STORAGE
    );
    assert_eq!(
        a.mutate("tenant", "new", "customer.create", "", body.clone())
            .await
            .expect_err("mutation fenced"),
        INCOMPATIBLE_STORAGE
    );
    assert_eq!(
        a.mutate("new-tenant", "new", "customer.create", "", body)
            .await
            .expect_err("insert fenced"),
        INCOMPATIBLE_STORAGE
    );
    let router = api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store: a,
        keys: Arc::new(BTreeMap::from([("key".into(), "tenant".into())])),
        demo: false,
        integrations: Arc::new(Integrations::default()),
    });
    for path in ["/health", "/v1/workspace"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", "Bearer key")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), 503, "{path}");
    }
    assert!(
        Store::new(Some(&db.url)).await.is_err(),
        "old startup must not downgrade format 20"
    );
    assert_eq!(
        canonical::physical(&db.pool, "tenant")
            .await
            .expect("unmodified future originals"),
        future_originals
    );
    let version: i32 =
        sqlx::query_scalar("SELECT version FROM freight_storage_format WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("version");
    assert_eq!(version, stateset_freight::store::STORAGE_FORMAT + 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_workspaces")
        .fetch_one(&db.pool)
        .await
        .expect("tenants");
    assert_eq!(count, 1, "failed insert and demo seeding leave no tenant");
    drop(router);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_prior_format_upgrades_retain_history_and_reject_previous_writers() {
    use sha2::{Digest, Sha256};
    for (previous_format, migration_count) in [
        (1, 3),
        (3, 5),
        (4, 6),
        (5, 7),
        (6, 8),
        (7, 9),
        (8, 10),
        (9, 11),
        (10, 12),
        (11, 13),
        (12, 14),
        (13, 15),
        (14, 16),
        (15, 17),
        (16, 18),
        (17, 22),
        (18, 23),
        (18, 24),
        (18, 25),
    ] {
        let db = Database::new().await;
        let mut tx = db.pool.begin().await.expect("previous release");
        sqlx::raw_sql("CREATE TABLE freight_schema_migrations(version integer PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())").execute(&mut *tx).await.expect("old history");
        for (index, sql) in PREVIOUS_MIGRATIONS.iter().take(migration_count).enumerate() {
            sqlx::raw_sql(sql)
                .execute(&mut *tx)
                .await
                .expect("previous migration");
            sqlx::query("INSERT INTO freight_schema_migrations(version,checksum) VALUES($1,$2)")
                .bind(index as i32 + 1)
                .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
                .execute(&mut *tx)
                .await
                .expect("old applied history");
        }
        let mut legacy = json!(domain::demo());
        legacy
            .as_object_mut()
            .expect("object")
            .remove("operations_cases");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("operations_policy");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("movement_corrections");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("cancellation_fees");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("source_checks");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("source_reviews");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("carrier_submissions");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("customer_quote_publications");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("customer_quote_decisions");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("document_shares");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("release_publications");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("release_responses");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("carrier_contracts");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("stop_services");
        legacy["_freight_format"] = json!(previous_format);
        sqlx::query("INSERT INTO freight_workspaces(tenant_id,data) VALUES('previous',$1)")
            .bind(&legacy)
            .execute(&mut *tx)
            .await
            .expect("previous persisted workspace");
        tx.commit().await.expect("old commit");
        let store = Store::new(Some(&db.url))
            .await
            .expect("upgrade to source review format");
        let mut upgraded = db.raw("previous").await;
        assert_eq!(
            upgraded["_freight_format"],
            stateset_freight::store::STORAGE_FORMAT
        );
        upgraded["_freight_format"] = json!(previous_format);
        assert_eq!(upgraded, legacy);
        assert!(store
            .read("previous")
            .await
            .expect("compatible read")
            .cancellation_fees
            .is_empty());
        assert!(store
            .read("previous")
            .await
            .expect("compatible read")
            .carrier_submissions
            .is_empty());
        let error = sqlx::query("UPDATE freight_workspaces SET data=$1 WHERE tenant_id='previous'")
            .bind(legacy)
            .execute(&db.pool)
            .await
            .expect_err("previous writer rejected");
        assert_eq!(
            error.as_database_error().and_then(|v| v.code()).as_deref(),
            Some("23514")
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_schema_migrations")
            .fetch_one(&db.pool)
            .await
            .expect("history");
        assert_eq!(count, 31);
        drop(store);
        db.clean().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_migration_refuses_unknown_row_format_and_rolls_back_ddl() {
    let db = Database::new().await;
    sqlx::raw_sql(include_str!("../migrations/001_workspaces.sql"))
        .execute(&db.pool)
        .await
        .expect("legacy table");
    let future = json!({"_freight_format":stateset_freight::store::STORAGE_FORMAT,"future_finance_data":{"amount_cents":1234567}});
    sqlx::query("INSERT INTO freight_workspaces(tenant_id,data) VALUES('future',$1)")
        .bind(&future)
        .execute(&db.pool)
        .await
        .expect("future data");
    assert!(Store::new(Some(&db.url)).await.is_err());
    assert_eq!(db.raw("future").await, future);
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname=current_schema() ORDER BY tablename",
    )
    .fetch_all(&db.pool)
    .await
    .expect("tables");
    assert_eq!(
        tables,
        ["freight_workspaces"],
        "failed migration rolls back content table and registry creation"
    );
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_upgrade_waits_for_inflight_writer_before_changing_format() {
    let db = Database::new().await;
    let a = Store::new(Some(&db.url)).await.expect("start");
    let mut writer = db.pool.begin().await.expect("inflight writer");
    // Match the API's lock order; a new release cannot change the registry
    // while a command is committing its workspace and related file bytes.
    sqlx::query("SELECT version FROM freight_storage_format WHERE singleton FOR SHARE")
        .execute(&mut *writer)
        .await
        .expect("writer fence");
    let mut upgrade = db.pool.begin().await.expect("upgrade");
    {
        let change = sqlx::query("UPDATE freight_storage_format SET version=22 WHERE singleton")
            .execute(&mut *upgrade);
        tokio::pin!(change);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut change)
                .await
                .is_err(),
            "upgrade must wait for inflight writer"
        );
        writer.commit().await.expect("finish writer");
        change.await.expect("upgrade resumes after commit");
    }
    upgrade.commit().await.expect("upgrade commit");
    assert!(!a.healthy().await);
    assert_eq!(
        a.mutate(
            "tenant",
            "blocked",
            "customer.create",
            "",
            json!({"name":"Blocked","contact":""})
        )
        .await
        .expect_err("writer fenced"),
        INCOMPATIBLE_STORAGE
    );
    drop(a);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_restart_skips_applied_migrations_and_refuses_modified_gapped_or_future_history() {
    let db = Database::new().await;
    let a = Store::new(Some(&db.url)).await.expect("start");
    a.seed_demo().await.expect("seed");
    let before = db.raw("demo").await;
    let history: Vec<(i32, String, String)> = sqlx::query_as(
        "SELECT version,checksum,applied_at::text FROM freight_schema_migrations ORDER BY version",
    )
    .fetch_all(&db.pool)
    .await
    .expect("migration history");
    assert_eq!(history.len(), 31);
    // A restart must not replay migration 003's exclusive tenant-table lock.
    let mut locked = db.pool.begin().await.expect("table lock");
    sqlx::query("LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locked)
        .await
        .expect("hold tenant table");
    let restarted =
        tokio::time::timeout(std::time::Duration::from_secs(2), Store::new(Some(&db.url)))
            .await
            .expect("restart must skip old table transformation")
            .expect("compatible restart");
    locked.rollback().await.expect("unlock");
    let unchanged: Vec<(i32, String, String)> = sqlx::query_as(
        "SELECT version,checksum,applied_at::text FROM freight_schema_migrations ORDER BY version",
    )
    .fetch_all(&db.pool)
    .await
    .expect("unchanged history");
    assert_eq!(unchanged, history);

    sqlx::query("UPDATE freight_schema_migrations SET checksum=repeat('0',64) WHERE version=1")
        .execute(&db.pool)
        .await
        .expect("simulate edited history");
    assert!(Store::new(Some(&db.url)).await.is_err());
    sqlx::query("UPDATE freight_schema_migrations SET checksum=$1 WHERE version=1")
        .bind(&history[0].1)
        .execute(&db.pool)
        .await
        .expect("restore checksum");
    sqlx::query("DELETE FROM freight_schema_migrations WHERE version=2")
        .execute(&db.pool)
        .await
        .expect("simulate gap");
    assert!(Store::new(Some(&db.url)).await.is_err());
    sqlx::query("INSERT INTO freight_schema_migrations(version,checksum) VALUES(2,$1)")
        .bind(&history[1].1)
        .execute(&db.pool)
        .await
        .expect("restore owned history");
    sqlx::query(
        "INSERT INTO freight_schema_migrations(version,checksum) VALUES($1,repeat('f',64))",
    )
    .bind(history.last().expect("current migration").0 + 1)
    .execute(&db.pool)
    .await
    .expect("simulate future release");
    assert!(Store::new(Some(&db.url)).await.is_err());
    assert_eq!(db.raw("demo").await, before);
    drop((a, restarted));
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_resolution_upgrade_refuses_ambiguous_original_receipt_extensions() {
    use sha2::{Digest, Sha256};
    let db = Database::new().await;
    sqlx::raw_sql("CREATE TABLE freight_schema_migrations(version integer PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())").execute(&db.pool).await.expect("original ledger");
    for (index, sql) in PREVIOUS_MIGRATIONS.iter().take(22).enumerate() {
        sqlx::raw_sql(sql)
            .execute(&db.pool)
            .await
            .expect("format-17 migration");
        sqlx::query("INSERT INTO freight_schema_migrations(version,checksum) VALUES($1,$2)")
            .bind(index as i32 + 1)
            .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
            .execute(&db.pool)
            .await
            .expect("original checksums");
    }
    let mut original = json!(domain::demo());
    original["_freight_format"] = json!(17);
    original["receipts"] = json!({"original-key":{"fingerprint":"original-request","response":{"original_evidence":true},"stopped":{"legacy_original_extension":true}}});
    sqlx::query("INSERT INTO freight_workspaces(tenant_id,data) VALUES('original',$1)")
        .bind(&original)
        .execute(&db.pool)
        .await
        .expect("legacy extension");
    assert!(
        Store::new(Some(&db.url)).await.is_err(),
        "original unknown data cannot become a new stop decision"
    );
    assert_eq!(db.raw("original").await, original);
    let version: i32 =
        sqlx::query_scalar("SELECT version FROM freight_storage_format WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("original format");
    assert_eq!(version, 17);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_schema_migrations")
        .fetch_one(&db.pool)
        .await
        .expect("original history");
    assert_eq!(count, 22);
    db.clean().await;
}
