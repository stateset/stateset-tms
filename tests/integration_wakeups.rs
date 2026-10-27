#[path = "support/canonical.rs"]
mod canonical;
use axum::{
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::PgPool;
use stateset_freight::{
    domain,
    integrations::{Connection, Integrations},
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Notify;

// Migrations deliberately share a database-wide advisory lock. Independent
// fixture startup is serialized; competing workers inside each scenario are not.
static DATABASE_SCENARIO: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn integration(url: &str) -> Integrations {
    Integrations {
        tenants: BTreeMap::from([(
            "demo".into(),
            Connection {
                edi_url: Some(url.into()),
                edi_token: Some("owned-test-token".into()),
                edi_tenant_id: Some("owned-test-tenant".into()),
                ..Default::default()
            },
        )]),
    }
}

#[tokio::test]
async fn idle_memory_queues_preserve_unreconciled_state_and_never_create_unknown_tenants() {
    let store = Store::new(None).await.expect("memory");
    store.seed_demo().await.expect("seed");
    let before = json!(store.read("demo").await.expect("before"));
    let integrations = integration("http://127.0.0.1:9");
    for tenant in ["demo", "unknown"] {
        assert!(!integrations
            .process_inbound_one(&store, tenant)
            .await
            .expect("idle inbox"));
        assert!(!integrations
            .dispatch_one(&store, tenant)
            .await
            .expect("idle outbox"));
    }
    assert_eq!(json!(store.read("demo").await.expect("after")), before);
    assert_eq!(store.tenant_ids().await.expect("tenants"), vec!["demo"]);
}

struct Database {
    _scenario: tokio::sync::MutexGuard<'static, ()>,
    admin: PgPool,
    pool: PgPool,
    schema: String,
    url: String,
}
impl Database {
    async fn new() -> Self {
        let scenario = DATABASE_SCENARIO.lock().await;
        let base = std::env::var("TEST_DATABASE_URL").expect("owned disposable database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("queue_test_{}", uuid::Uuid::new_v4().simple());
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
            _scenario: scenario,
            admin,
            pool,
            schema,
            url: url.to_string(),
        }
    }
    async fn raw(&self) -> Value {
        let canonical_storage: bool =
            sqlx::query_scalar("SELECT to_regclass('freight_workspace_blocks') IS NOT NULL")
                .fetch_one(&self.pool)
                .await
                .expect("fixture representation");
        let mut row: Value = sqlx::query_scalar("SELECT jsonb_build_object('data',data,'updated_at',updated_at,'xmin',xmin::text,'ctid',ctid::text,'inbox_due_at',inbox_due_at,'edi_due_at',edi_due_at,'sync_due_at',sync_due_at) FROM freight_workspaces WHERE tenant_id='demo'")
            .fetch_one(&self.pool).await.expect("complete row");
        if canonical_storage {
            row["data"] = canonical::source(&self.pool, "demo")
                .await
                .expect("canonical originals");
            row["physical_originals"] = canonical::physical(&self.pool, "demo")
                .await
                .expect("canonical physical originals");
        }
        row
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
async fn postgres_queue_deadlines_backfill_original_data_and_match_retry_and_lease_semantics() {
    let db = Database::new().await;
    // Install the former schema, then add scheduling metadata to actual rows.
    for sql in [
        include_str!("../migrations/001_workspaces.sql"),
        include_str!("../migrations/002_documents.sql"),
    ] {
        sqlx::raw_sql(sql)
            .execute(&db.pool)
            .await
            .expect("legacy schema");
    }
    let mut original = json!(domain::demo());
    original["future_retained_field"] = json!({"original":"preserve exactly"});
    original["inbox"] = json!([{"status":"retry","next_attempt_at":"2027-01-01T01:00:00+01:00"}]);
    sqlx::query("INSERT INTO freight_workspaces(tenant_id,data,updated_at) VALUES('demo',$1,'2026-01-01T00:00:00Z')").bind(&original).execute(&db.pool).await.expect("original");
    sqlx::raw_sql(include_str!("../migrations/020_integration_wakeups.sql"))
        .execute(&db.pool)
        .await
        .expect("derived-only migration");
    let row = db.raw().await;
    assert_eq!(row["data"], original);
    assert_eq!(row["updated_at"], "2026-01-01T00:00:00+00:00");
    assert_eq!(row["inbox_due_at"], "2027-01-01T00:00:00+00:00");
    // Protect metadata even against an explicit stale/manual column update.
    sqlx::query("UPDATE freight_workspaces SET inbox_due_at='infinity'")
        .execute(&db.pool)
        .await
        .expect("metadata refresh");
    assert_eq!(db.raw().await["inbox_due_at"], row["inbox_due_at"]);
    for (queue, inbound, destination, expected) in [
        (json!([]), true, None, None),
        (json!([{"status":"applied"}]), true, None, None),
        (json!([{"status":"pending"}]), true, None, Some("-infinity")),
        (
            json!([{"status":"retry","next_attempt_at":null}]),
            true,
            None,
            Some("-infinity"),
        ),
        (
            json!([{"status":"processing","leased_until":null}]),
            true,
            None,
            None,
        ),
        (
            json!([{"status":"processing","leased_until":"2027-01-02T00:00:00Z"}]),
            true,
            None,
            Some("2027-01-02 00:00:00+00"),
        ),
        (
            json!([{"status":"retry","next_attempt_at":"2027-01-01T00:00:00Z"},{"status":"processing","leased_until":"2027-01-02T00:00:00Z"}]),
            true,
            None,
            Some("2027-01-01 00:00:00+00"),
        ),
        (
            json!([{"status":"retry","next_attempt_at":"2027-02-30T00:00:00Z"}]),
            true,
            None,
            Some("-infinity"),
        ),
        (
            json!([{"status":"retry","next_attempt_at":"2027-01-01T00:00:00+99:99"}]),
            true,
            None,
            Some("-infinity"),
        ),
        (
            json!([{"status":"retry","next_attempt_at":"2027-01-01"}]),
            true,
            None,
            Some("-infinity"),
        ),
        (
            json!([{"status":"retry","next_attempt_at":5}]),
            true,
            None,
            Some("-infinity"),
        ),
        (json!({"damaged":"retained"}), true, None, Some("-infinity")),
        (json!([{"status":false}]), true, None, Some("-infinity")),
        (
            json!([{"status":"pending_handoff","destination":"stateset-edi"}]),
            false,
            Some("stateset-edi"),
            Some("-infinity"),
        ),
        (
            json!([{"status":"pending_handoff","destination":"stateset-sync-server"}]),
            false,
            Some("stateset-edi"),
            None,
        ),
        (
            json!([{"status":"held","destination":"stateset-edi"}]),
            false,
            Some("stateset-edi"),
            None,
        ),
        (
            json!([{"status":"sending","destination":"stateset-edi","leased_until":null}]),
            false,
            Some("stateset-edi"),
            None,
        ),
        (
            json!([{"status":"sending","destination":"stateset-edi","leased_until":"2027-01-01T00:00:00-02:00"}]),
            false,
            Some("stateset-edi"),
            Some("2027-01-01 02:00:00+00"),
        ),
    ] {
        let actual: Option<String> = sqlx::query_scalar("SELECT freight_queue_due($1,$2,$3)::text")
            .bind(&queue)
            .bind(inbound)
            .bind(destination)
            .fetch_one(&db.pool)
            .await
            .expect("actual scheduler");
        assert_eq!(actual.as_deref(), expected, "{queue}");
    }
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_idle_polls_avoid_aggregate_access_and_competing_claims_and_late_completions_do_not_write(
) {
    let db = Database::new().await;
    let a = Store::new(Some(&db.url)).await.expect("first store");
    let b = Store::new(Some(&db.url)).await.expect("second store");
    a.seed_demo().await.expect("seed");
    sqlx::raw_sql("CREATE TABLE queue_writes(id bigserial PRIMARY KEY); CREATE FUNCTION queue_trace() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO queue_writes DEFAULT VALUES; RETURN NEW; END $$; CREATE TRIGGER queue_trace AFTER UPDATE OF data ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION queue_trace();").execute(&db.pool).await.expect("physical observer");
    let integrations = integration("http://127.0.0.1:9");
    let before = db.raw().await;
    let mut lock = db.pool.begin().await.expect("lock");
    sqlx::query("SELECT tenant_id FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE")
        .execute(&mut *lock)
        .await
        .expect("hold tenant writer");
    for _ in 0..4 {
        tokio::time::timeout(Duration::from_secs(2), async {
            assert!(!integrations
                .process_inbound_one(&a, "demo")
                .await
                .expect("idle inbox"));
            assert!(!integrations
                .dispatch_one(&a, "demo")
                .await
                .expect("idle outbox"));
        })
        .await
        .expect("idle polls must not wait on a tenant writer");
    }
    lock.rollback().await.expect("unlock");
    assert_eq!(db.raw().await, before);
    // An unrelated unsupported root demonstrates that idle scheduling performs
    // no whole-aggregate decode. Business reads still fail closed unchanged.
    let intact = canonical::source(&db.pool, "demo")
        .await
        .expect("intact originals");
    let mut unsupported = intact.clone();
    unsupported["unknown_retained_root"] = json!(true);
    canonical::put(&db.pool, "demo", &unsupported)
        .await
        .expect("retained unsupported data");
    let retained = db.raw().await;
    assert!(a.read("demo").await.is_err());
    assert!(!integrations
        .process_inbound_one(&a, "demo")
        .await
        .expect("no business work"));
    assert!(!integrations
        .dispatch_one(&a, "demo")
        .await
        .expect("no business work"));
    assert_eq!(db.raw().await, retained);
    canonical::put(&db.pool, "demo", &intact)
        .await
        .expect("restore exact original");

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let app = Router::new().route("/v1/documents/owned-990",get({
        let started=started.clone(); let release=release.clone();
        move || { let started=started.clone(); let release=release.clone(); async move {
            started.notify_one(); release.notified().await;
            Json(json!({"data":{"id":"owned-990","direction":"inbound","transaction_set":"990","status":"accepted","partner_id":"brex","processed_at":Utc::now(),"parsed_data":{"shipment_id":"SFL-2026-0103","carrier_scac":"BREX","accepted":true}}}))
        }}
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let integrations = integration(&format!("http://{address}"));
    // Hold an actual HTTP fetch while another instance reclaims its lease.
    let note = json!({"type":"document.recorded","tenant_id":"owned-test-tenant","document_id":"owned-990","direction":"inbound","status":"accepted","transaction_set":"990","partner_id":"brex"});
    a.receive_document(
        "demo",
        "owned-delivery",
        "1",
        note.clone(),
        note.to_string(),
        None,
    )
    .await
    .expect("durable callback");
    let mut lock = db.pool.begin().await.expect("serialize competing claims");
    sqlx::query("SELECT tenant_id FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE")
        .execute(&mut *lock)
        .await
        .expect("hold current tenant");
    let (store, connection) = (a.clone(), integrations.clone());
    let mut first =
        tokio::spawn(async move { connection.process_inbound_one(&store, "demo").await });
    let (store, connection) = (b.clone(), integrations.clone());
    let mut second =
        tokio::spawn(async move { connection.process_inbound_one(&store, "demo").await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let blocked: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock'")
            .bind(&db.schema).fetch_one(&db.pool).await.expect("actual competing row locks");
        if blocked >= 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "both workers must pass the positive probe and reach the tenant lock"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    lock.commit().await.expect("release competing workers");
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("actual HTTP fetch claimed");
    let (loser, winner) = tokio::select! {
        result = &mut first => (result, second),
        result = &mut second => (result, first),
    };
    assert!(!loser
        .expect("losing worker")
        .expect("claim rechecked under lock"));
    let claimed = db.raw().await;
    assert_eq!(claimed["data"]["inbox"][0]["status"], "processing");
    assert!(!integrations
        .process_inbound_one(&b, "demo")
        .await
        .expect("live lease excludes contender"));
    assert_eq!(db.raw().await, claimed);
    // Expired leases remain reclaimable. Alter the canonical notification to
    // an ignored direction so the second claim completes without external IO.
    let mut reclaimed = canonical::source(&db.pool, "demo")
        .await
        .expect("claimed source");
    reclaimed["inbox"][0]["leased_until"] = json!(Utc::now() - chrono::Duration::minutes(1));
    reclaimed["inbox"][0]["notification"]["direction"] = json!("outbound");
    canonical::put(&db.pool, "demo", &reclaimed)
        .await
        .expect("owned crash simulation");
    assert!(integrations
        .process_inbound_one(&b, "demo")
        .await
        .expect("reclaim"));
    let completed = db.raw().await;
    assert_eq!(completed["data"]["inbox"][0]["status"], "ignored");
    assert_eq!(completed["data"]["inbox"][0]["total_attempts"], 2);
    release.notify_one();
    winner.await.expect("old worker").expect("old lease fenced");
    assert_eq!(
        db.raw().await,
        completed,
        "late completion must not update even the physical row"
    );
    assert!(completed["inbox_due_at"].is_null());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM queue_writes")
        .fetch_one(&db.pool)
        .await
        .expect("physical updates");
    assert_eq!(count,7,"only two owned corruption/restoration writes, receipt, claim, lease simulation and reclaimed completion transactions");
    server.abort();
    drop((a, b));
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_outbound_deadlines_respect_future_retries_and_expired_carrier_coordination() {
    let db = Database::new().await;
    let a = Store::new(Some(&db.url)).await.expect("first store");
    let b = Store::new(Some(&db.url)).await.expect("second store");
    a.seed_demo().await.expect("seed");
    a.mutate(
        "demo",
        "owned-tender",
        "load.command",
        "load_5",
        json!({"action":"tender","expected_version":1,"carrier_id":"ca2"}),
    )
    .await
    .expect("original tender");
    let mut retry = canonical::source(&db.pool, "demo")
        .await
        .expect("tender source");
    retry["outbox"][0]["status"] = json!("retry");
    retry["outbox"][0]["next_attempt_at"] = json!(Utc::now() + chrono::Duration::hours(1));
    canonical::put(&db.pool, "demo", &retry)
        .await
        .expect("future retry");
    let integrations = integration("http://127.0.0.1:9");
    let future = db.raw().await;
    assert!(!future["edi_due_at"].is_null());
    assert!(future["sync_due_at"].is_null());
    assert!(!integrations
        .dispatch_one(&a, "demo")
        .await
        .expect("future work sleeps"));
    assert_eq!(db.raw().await, future);
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let app = Router::new().route(
        "/v1/edi/outbound/204/pclg",
        post({
            let started = started.clone();
            let release = release.clone();
            move || {
                let started = started.clone();
                let release = release.clone();
                async move {
                    started.notify_one();
                    release.notified().await;
                    "ST*204*OWNED~"
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let integrations = integration(&format!("http://{address}"));
    canonical::set(
        &db.pool,
        "demo",
        &["outbox", "0", "next_attempt_at"],
        Value::Null,
    )
    .await
    .expect("due retry");
    let (store, connection) = (a.clone(), integrations.clone());
    let first = tokio::spawn(async move { connection.dispatch_one(&store, "demo").await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("original handoff started");
    let sending = db.raw().await;
    assert_eq!(sending["data"]["outbox"][0]["total_attempts"], 1);
    assert!(!integrations
        .dispatch_one(&b, "demo")
        .await
        .expect("live lease"));
    assert_eq!(db.raw().await, sending);
    let load = a.read("demo").await.expect("current source").loads[4].clone();
    a.mutate("demo","owned-release","tender.release.request","load_5",json!({"expected_load_version":load.version,"disposition":"replan","reason":"Actual carrier coordination before replacement"})).await.expect("request release");
    canonical::set(
        &db.pool,
        "demo",
        &["outbox", "0", "leased_until"],
        json!(Utc::now() - chrono::Duration::minutes(1)),
    )
    .await
    .expect("expired handoff");
    assert!(!integrations
        .dispatch_one(&b, "demo")
        .await
        .expect("fence uncertain handoff"));
    let held = db.raw().await;
    assert_eq!(held["data"]["outbox"][0]["status"], "held");
    assert_eq!(held["data"]["outbox"][0]["transmission"], "attempted");
    assert_eq!(
        held["data"]["outbox"][0]["payload"],
        sending["data"]["outbox"][0]["payload"]
    );
    assert!(held["edi_due_at"].is_null());
    release.notify_one();
    assert!(first
        .await
        .expect("old handoff")
        .expect("old completion fenced"));
    assert_eq!(
        db.raw().await,
        held,
        "late handoff completion must not rewrite the held row"
    );
    assert!(!integrations
        .dispatch_one(&b, "demo")
        .await
        .expect("held work sleeps"));
    assert_eq!(db.raw().await, held);
    let restarted = Store::new(Some(&db.url)).await.expect("restart");
    assert!(!integrations
        .dispatch_one(&restarted, "demo")
        .await
        .expect("retained deadline after reconnect"));
    assert_eq!(db.raw().await, held);
    server.abort();
    drop((a, b, restarted));
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_queue_probes_use_the_claim_clock_when_the_database_clock_differs() {
    let db = Database::new().await;
    // Shadow the SQL clock only inside this owned schema/search path. Real
    // Freight timestamps and worker claims still use the process UTC clock.
    sqlx::raw_sql("CREATE FUNCTION now() RETURNS timestamptz LANGUAGE sql STABLE AS $$ SELECT '2000-01-01T00:00:00Z'::timestamptz $$").execute(&db.pool).await.expect("owned database clock fixture");
    let store = Store::new(Some(&db.url)).await.expect("store");
    store.seed_demo().await.expect("seed");
    let database_clock: String = sqlx::query_scalar("SELECT now()::text")
        .fetch_one(&db.pool)
        .await
        .expect("database clock");
    assert!(database_clock.starts_with("2000-01-01"));
    let note = json!({"type":"document.recorded","tenant_id":"owned-test-tenant","document_id":"clock-ignored","direction":"outbound","status":"accepted","transaction_set":"990","partner_id":"pclg"});
    store
        .receive_document(
            "demo",
            "clock-delivery",
            "1",
            note.clone(),
            note.to_string(),
            None,
        )
        .await
        .expect("receipt");
    store
        .mutate(
            "demo",
            "clock-tender",
            "load.command",
            "load_5",
            json!({"action":"tender","expected_version":1,"carrier_id":"ca2"}),
        )
        .await
        .expect("tender");
    let due = json!(Utc::now() - chrono::Duration::minutes(1));
    let mut source = canonical::source(&db.pool, "demo")
        .await
        .expect("queued source");
    source["inbox"][0]["next_attempt_at"] = due.clone();
    source["outbox"][0]["next_attempt_at"] = due;
    canonical::put(&db.pool, "demo", &source)
        .await
        .expect("ready work by the claim clock");
    let incorrect: (bool,bool)=sqlx::query_as("SELECT inbox_due_at <= now(),edi_due_at <= now() FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&db.pool).await.expect("old database-clock predicate");
    assert_eq!(
        incorrect,
        (false, false),
        "this fixture must expose the former clock mismatch"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let address = listener.local_addr().expect("address");
    let app = Router::new().route(
        "/v1/edi/outbound/204/pclg",
        post(|| async { "ISA*owned clock fixture~ST*204*OWNED-CLOCK~" }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let integrations = integration(&format!("http://{address}"));
    assert!(integrations
        .process_inbound_one(&store, "demo")
        .await
        .expect("ready inbound must run"));
    assert!(integrations
        .dispatch_one(&store, "demo")
        .await
        .expect("ready outbound must run"));
    let after = db.raw().await;
    assert_eq!(after["data"]["inbox"][0]["status"], "ignored");
    assert_eq!(after["data"]["outbox"][0]["status"], "gateway_accepted");
    assert_eq!(
        after["data"]["outbox"][0]["response"],
        "ISA*owned clock fixture~ST*204*OWNED-CLOCK~"
    );
    assert!(after["inbox_due_at"].is_null() && after["edi_due_at"].is_null());
    server.abort();
    drop(store);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_native_commerce_wakeups_preserve_originals_and_cover_pending_held_and_expired_leases(
) {
    let db = Database::new().await;
    let store = Store::new(Some(&db.url)).await.expect("canonical schema");
    store.seed_demo().await.expect("seed");
    let mut original = domain::demo();
    let mut job:domain::Outbox=serde_json::from_value(json!({"id":"owned-native-wakeup","load_id":original.loads[0].id,"destination":"stateset-icommerce","status":"pending_handoff","method":"PATCH","path":"/api/v1/shipments/00000000-0000-4000-8000-000000000005","payload":{"expected_version":3,"status":"in_transit"}})).expect("owned native outbox");
    job.destination = "stateset-icommerce".into();
    job.status = "pending_handoff".into();
    job.next_attempt_at = None;
    job.leased_until = None;
    original.outbox = vec![job.clone()];
    canonical::put_workspace(&db.pool, "demo", &original)
        .await
        .expect("native fixture");
    // Restore the preceding projection with a stale future deadline, then prove
    // migration 029 changes only derived wakeup metadata, not business revision.
    let legacy = include_str!("../migrations/020_integration_wakeups.sql");
    let function = legacy
        .split("CREATE FUNCTION freight_refresh_integration_wakeups")
        .next()
        .expect("function source")
        .split("CREATE FUNCTION freight_queue_due")
        .nth(1)
        .expect("function");
    sqlx::raw_sql(&format!(
        "CREATE OR REPLACE FUNCTION freight_queue_due{function}"
    ))
    .execute(&db.pool)
    .await
    .expect("preceding queue projection");
    sqlx::query("UPDATE freight_workspaces SET sync_due_at=NULL WHERE tenant_id='demo'")
        .execute(&db.pool)
        .await
        .expect("preceding native deadline absent");
    let before = db.raw().await;
    assert!(before["sync_due_at"].is_null());
    let revision: i64 = sqlx::query_scalar(
        "SELECT history_revision FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("original revision");
    let mut migration = db.pool.begin().await.expect("owned migration transaction");
    sqlx::raw_sql(include_str!("../migrations/029_commerce_wakeups.sql"))
        .execute(&mut *migration)
        .await
        .expect("repeat derived projection");
    migration.commit().await.expect("atomic derived projection");
    let after = db.raw().await;
    assert_eq!(after["sync_due_at"], "-infinity");
    let after_revision: i64 = sqlx::query_scalar(
        "SELECT history_revision FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("retained revision");
    assert_eq!(after_revision, revision);
    assert_eq!(before["data"], after["data"]);
    for field in ["blocks", "counts", "header", "updated_at", "revision"] {
        assert_eq!(
            before["physical_originals"][field], after["physical_originals"][field],
            "preserve original {field}; only the scheduling row tuple may change"
        );
    }
    assert_eq!(before["updated_at"], after["updated_at"]);
    for (status, next, lease, expected) in [
        ("held", None, None, None),
        ("synced", None, None, None),
        (
            "retry",
            Some("2027-01-01T00:00:00Z"),
            None,
            Some("2027-01-01 00:00:00+00"),
        ),
        (
            "sending",
            None,
            Some("2026-01-01T00:00:00Z"),
            Some("2026-01-01 00:00:00+00"),
        ),
    ] {
        job.status = status.into();
        job.next_attempt_at = next.map(|v| v.parse().expect("time"));
        job.leased_until = lease.map(|v| v.parse().expect("time"));
        let due: Option<String> =
            sqlx::query_scalar("SELECT freight_queue_due($1,false,'stateset-sync-server')::text")
                .bind(json!([job]))
                .fetch_one(&db.pool)
                .await
                .expect("native scheduling");
        assert_eq!(due.as_deref(), expected);
    }
    job.status = "sending".into();
    job.transmission = "attempted".into();
    job.lease = Some("original-expired-native-lease".into());
    job.leased_until = Some(Utc::now() - chrono::Duration::seconds(1));
    original.outbox = vec![job.clone()];
    canonical::put_workspace(&db.pool, "demo", &original)
        .await
        .expect("expired native fixture");
    let native = Integrations {
        tenants: BTreeMap::from([(
            "demo".into(),
            Connection {
                commerce_url: Some("http://127.0.0.1:9".into()),
                commerce_token: Some("owned-native-credential-at-least-32-characters".into()),
                commerce_tenant_id: Some("native".into()),
                ..Default::default()
            },
        )]),
    };
    assert!(!native
        .dispatch_one(&store, "demo")
        .await
        .expect("expired native held without network retry"));
    let held = store.read("demo").await.expect("held original");
    let held = held
        .outbox
        .iter()
        .find(|v| v.id == job.id)
        .expect("original job");
    assert_eq!(held.status, "held");
    assert_eq!(held.payload, job.payload);
    assert_eq!(held.transmission, "attempted");
    assert!(held.lease.is_none());
    drop(store);
    db.clean().await;
}
