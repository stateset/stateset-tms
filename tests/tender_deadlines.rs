#[path = "support/canonical.rs"]
mod canonical;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use stateset_freight::{domain, store::Store};

struct Database {
    admin: PgPool,
    pool: PgPool,
    schema: String,
    url: String,
}
impl Database {
    async fn new(previous: bool) -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("deadline_{}", uuid::Uuid::new_v4().simple());
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
            let mut tx = pool.begin().await.expect("previous schema");
            sqlx::raw_sql("CREATE TABLE freight_schema_migrations(version integer PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())").execute(&mut *tx).await.expect("ledger");
            let mut files: Vec<_> =
                std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
                    .expect("immutable migrations")
                    .map(|p| p.expect("entry").path())
                    .filter(|p| p.extension().is_some_and(|s| s == "sql"))
                    .collect();
            files.sort();
            for (i, p) in files.iter().take(24).enumerate() {
                assert!(p
                    .file_name()
                    .expect("name")
                    .to_string_lossy()
                    .starts_with(&format!("{:03}_", i + 1)));
                let sql = std::fs::read_to_string(p).expect("previous migration");
                sqlx::raw_sql(&sql)
                    .execute(&mut *tx)
                    .await
                    .expect("previous SQL");
                sqlx::query(
                    "INSERT INTO freight_schema_migrations(version,checksum) VALUES($1,$2)",
                )
                .bind(i as i32 + 1)
                .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
                .execute(&mut *tx)
                .await
                .expect("old checksum");
            }
            tx.commit().await.expect("previous commit");
        }
        Self {
            admin,
            pool,
            schema,
            url: url.to_string(),
        }
    }
    async fn physical(&self) -> Value {
        canonical::physical(&self.pool, "demo")
            .await
            .expect("physical originals")
    }
    async fn raw(&self) -> Value {
        sqlx::query_scalar("SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id='demo'")
            .fetch_one(&self.pool)
            .await
            .expect("original source")
    }
    async fn put(&self, data: &Value) {
        canonical::put(&self.pool, "demo", data)
            .await
            .expect("owned fixture");
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
fn fixture() -> Value {
    let mut w = domain::demo();
    for load in &mut w.loads {
        load.tender_expires_at = None;
    }
    w.loads[2].exception = None;
    w.loads[2].tender_expires_at = Some(Utc::now() + Duration::hours(1));
    let mut value = json!(w);
    value["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    value
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_deadline_minor_upgrade_preserves_source_and_canonical_upgrade_fences_old_writer_with_safe_fallback(
) {
    let db = Database::new(true).await;
    let original = fixture();
    let mut previous = original.clone();
    previous["_freight_format"] = json!(18);
    sqlx::query("INSERT INTO freight_workspaces(tenant_id,data) VALUES('demo',$1)")
        .bind(&previous)
        .execute(&db.pool)
        .await
        .expect("preceding source");
    let before: (String,String,String,i64) = sqlx::query_as("SELECT xmin::text,updated_at::text,md5(data::text),history_revision FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("minor source before");
    let mut old = db.pool.acquire().await.expect("old writer");
    sqlx::query("PREPARE old_deadline_writer(jsonb) AS UPDATE freight_workspaces SET data=$1 WHERE tenant_id='demo'")
        .execute(&mut *old).await.expect("old prepared statement");
    canonical::minor_migration(
        &db.pool,
        25,
        include_str!("../migrations/025_tender_deadlines.sql"),
    )
    .await
    .expect("deadline-only upgrade");
    let after: (String,String,String,i64) = sqlx::query_as("SELECT xmin::text,updated_at::text,md5(data::text),history_revision FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("minor source after");
    assert_eq!(
        before, after,
        "minor deadline migration must not rewrite originals"
    );
    let literal = previous.to_string().replace('\'', "''");
    sqlx::raw_sql(&format!("EXECUTE old_deadline_writer('{literal}'::jsonb)"))
        .execute(&mut *old)
        .await
        .expect("minor migration retains prepared writer");
    let previous_stamp: (String, i64) = sqlx::query_as(
        "SELECT updated_at::text,history_revision FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("precanonical metadata");
    let (a, b) = tokio::join!(Store::new(Some(&db.url)), Store::new(Some(&db.url)));
    let store = a.expect("upgrade");
    drop(b.expect("concurrent upgrade"));
    let stamp: (String, i64) = sqlx::query_as(
        "SELECT updated_at::text,history_revision FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("backfilled metadata");
    assert_eq!(
        stamp, previous_stamp,
        "canonical backfill preserves time and revision"
    );
    let before = db.physical().await;
    assert_eq!(db.raw().await, original);
    assert_eq!(store.expire_tenders("demo").await.expect("quiet"), 0);
    assert_eq!(store.expire_tenders("missing").await.expect("absent"), 0);
    assert_eq!(db.physical().await, before);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_workspaces")
        .fetch_one(&db.pool)
        .await
        .expect("no new tenant");
    assert_eq!(count, 1);

    let mut changed = original.clone();
    // Quiet expiry deliberately does not certify unrelated retained models.
    // The ordinary workspace read still refuses the malformed history.
    changed["events"] = json!("owned malformed unrelated retained history");
    let literal = changed.to_string().replace('\'', "''");
    let refusal = sqlx::raw_sql(&format!("EXECUTE old_deadline_writer('{literal}'::jsonb)"))
        .execute(&mut *old)
        .await
        .expect_err("major upgrade refuses aggregate writer even with new marker");
    assert_eq!(
        refusal
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514")
    );
    db.put(&changed).await;
    assert!(store.read("demo").await.is_err());
    let before = db.physical().await;
    assert_eq!(
        store
            .expire_tenders("demo")
            .await
            .expect("quiet scalar preflight"),
        0
    );
    assert_eq!(db.physical().await, before);
    sqlx::query("DELETE FROM freight_tender_deadlines WHERE tenant_id='demo'")
        .execute(&db.pool)
        .await
        .expect("missing hint");
    assert_eq!(
        store
            .expire_tenders("demo")
            .await
            .expect("missing hint rebuilt"),
        0
    );
    assert_eq!(db.physical().await, before);

    let now = Utc::now();
    for nanos in [-1001, -1, 0, 1, 999, 1001] {
        let deadline = now + Duration::nanoseconds(nanos);
        let mut source = original.clone();
        source["loads"][2]["tender_expires_at"] = json!(deadline);
        let maybe: bool =
            sqlx::query_scalar("SELECT freight_next_tender_deadline($1)<=$2::text::timestamptz")
                .bind(&source)
                .bind(now.to_rfc3339())
                .fetch_one(&db.pool)
                .await
                .expect("precision hint");
        if deadline < now {
            assert!(
                maybe,
                "nanosecond deadlines cannot be hidden by PostgreSQL rounding"
            );
        }
    }
    let mut leap = original.clone();
    leap["loads"][2]["tender_expires_at"] = json!("2026-12-31T23:59:60.9Z");
    let conservative: bool =
        sqlx::query_scalar("SELECT freight_next_tender_deadline($1)='-infinity'::timestamptz")
            .bind(&leap)
            .fetch_one(&db.pool)
            .await
            .expect("leap-second fallback");
    assert!(
        conservative,
        "fractional leap seconds require exact native interpretation"
    );
    for value in [
        json!({"bad":"deadline"}),
        json!("not a timestamp"),
        json!("2026-02-30T00:00:00Z"),
    ] {
        let mut bad = original.clone();
        bad["loads"][2]["tender_expires_at"] = value;
        db.put(&bad).await;
        let before = db.physical().await;
        assert!(
            store.expire_tenders("demo").await.is_err(),
            "malformed expiry input must force the full typed check"
        );
        assert_eq!(db.physical().await, before);
    }
    db.put(&original).await;
    canonical::corrupt_marker(&db.pool, "demo", "19.0")
        .await
        .expect("owned decimal marker damage");
    assert_eq!(
        store
            .expire_tenders("demo")
            .await
            .expect_err("exact source marker fence"),
        stateset_freight::store::INCOMPATIBLE_STORAGE
    );
    db.put(&original).await;
    sqlx::query("SELECT freight_ensure_tender_deadline('demo')")
        .execute(&db.pool)
        .await
        .expect("warm metadata");
    let plan: Value = sqlx::query_scalar(&format!(
        "EXPLAIN (ANALYZE,BUFFERS,VERBOSE,FORMAT JSON) {}",
        include_str!("../src/tender_expiry_due.sql")
    ))
    .bind("demo")
    .bind(Utc::now().to_rfc3339())
    .fetch_one(&db.pool)
    .await
    .expect("actual scalar plan");
    assert!(
        !plan.to_string().contains("w.data"),
        "scalar plan must not extract the source payload"
    );
    if let Ok(path) = std::env::var("FREIGHT_DEADLINE_PLAN_EVIDENCE") {
        std::fs::write(path, serde_json::to_vec_pretty(&plan).expect("plan JSON"))
            .expect("plan evidence");
    }
    drop(old);
    drop(store);
    let store = Store::new(Some(&db.url)).await.expect("restart");
    assert_eq!(
        store.expire_tenders("demo").await.expect("restarted hint"),
        0
    );
    drop(store);
    db.clean().await;
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_deadline_races_recheck_acceptance_and_expire_once_without_source_writes_on_idle_or_overflow(
) {
    let db = Database::new(false).await;
    let a = Store::new(Some(&db.url)).await.expect("first store");
    let b = Store::new(Some(&db.url)).await.expect("second store");
    let mut due = fixture();
    due["loads"][2]["tender_expires_at"] = json!(Utc::now() - Duration::hours(1));
    db.put(&due).await;
    sqlx::query("SELECT freight_ensure_tender_deadline('demo')")
        .execute(&db.pool)
        .await
        .expect("warm due hint");
    let mut blocker = db.pool.begin().await.expect("writer");
    sqlx::query("SELECT 1 FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("hold tenant");
    let reader = {
        let a = a.clone();
        tokio::spawn(async move { a.expire_tenders("demo").await })
    };
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock'")
            .bind(&db.schema).fetch_one(&db.admin).await.expect("actual blocked mutation");
        if n > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "expiry never reached its guarded write"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let mut accepted = due.clone();
    accepted["loads"][2]["status"] = json!("accepted");
    canonical::put_tx(&mut blocker, "demo", &accepted)
        .await
        .expect("late committed acceptance");
    blocker.commit().await.expect("acceptance commit");
    assert_eq!(reader.await.expect("expiry task").expect("safe recheck"), 0);
    let accepted = db.raw().await;
    assert_eq!(accepted["loads"][2]["exception"], Value::Null);
    assert_eq!(accepted["events"], due["events"]);
    db.put(&due).await;
    let (one, two) = tokio::join!(a.expire_tenders("demo"), b.expire_tenders("demo"));
    assert_eq!(one.expect("first") + two.expect("second"), 1);
    let expired = db.raw().await;
    assert_eq!(
        expired["loads"][2]["version"],
        due["loads"][2]["version"].as_u64().expect("version") + 1
    );
    assert_eq!(
        expired["events"]
            .as_array()
            .expect("events")
            .iter()
            .filter(|v| v["kind"] == "tender.overdue")
            .count(),
        1
    );
    let before = db.physical().await;
    assert_eq!(
        a.expire_tenders("demo").await.expect("idle after expiry"),
        0
    );
    assert_eq!(db.physical().await, before);
    due["loads"][2]["version"] = json!(u64::MAX);
    db.put(&due).await;
    let before = db.physical().await;
    assert!(a
        .expire_tenders("demo")
        .await
        .expect_err("overflow")
        .contains("version exhausted"));
    assert_eq!(db.physical().await, before);
    assert_eq!(db.raw().await, due);
    drop((a, b));
    db.clean().await;
}
