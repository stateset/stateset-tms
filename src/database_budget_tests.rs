use super::*;
use crate::{
    api::{self, App},
    integrations::Integrations,
};
use std::time::Duration;

struct Fixture {
    admin: PgPool,
    observer: PgPool,
    store: Store,
    url: String,
    schema: String,
    app: String,
    origin: String,
    server: tokio::task::JoinHandle<()>,
    client: reqwest::Client,
}

#[test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
fn postgres_cancelled_apply_retains_actual_locks_until_cpu_ends_then_original_retries_once() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("one async worker");
    runtime.block_on(async {
        let f = Fixture::new().await;
        let original_body =
            json!({"name":"Original before held apply","contact":"owned@example.test"});
        let original = f
            .store
            .mutate(
                "demo",
                "cpu-original",
                "customer.create",
                "",
                original_body.clone(),
            )
            .await
            .expect("original receipt");
        let before = f.physical().await;
        let body =
            json!({"name":"Exactly one explicit retry","contact":"owned-retry@example.test"});
        let signature = fingerprint("customer.create", "", &body);
        let apply_signature = signature.clone();
        let apply_body = body.clone();
        let gate = Arc::new(super::mutation_execution_tests::WorkGate::default());
        let worker_gate = gate.clone();
        let (entered, observed) = tokio::sync::oneshot::channel();
        let store = f.store.clone();
        let writer = tokio::spawn(async move {
            store
                .update_receipted_original(
                    "demo",
                    None,
                    None,
                    Some(("cpu-cancelled", &signature)),
                    move |workspace| {
                        let result = apply(
                            workspace,
                            "cpu-cancelled",
                            &apply_signature,
                            "customer.create",
                            "",
                            apply_body,
                            ("owned-cpu", false),
                        )?;
                        entered.send(()).expect("actual apply entered");
                        worker_gate.hold();
                        Ok(result)
                    },
                )
                .await
        });
        observed.await.expect("actual apply observed");
        writer.abort();
        assert!(writer.await.expect_err("cancelled caller").is_cancelled());
        assert_eq!(
            f.store.write_turns.requests(),
            1,
            "actual abandoned work still owns its write turn"
        );
        assert_eq!(f.get("/live").await.status(), 200);
        assert_eq!(f.get("/ready").await.status(), 200);
        let locked = sqlx::query(
            "SELECT tenant_id FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE NOWAIT",
        )
        .fetch_optional(&f.observer)
        .await
        .expect_err("worker retains actual tenant row lock");
        assert_eq!(
            locked
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("55P03")
        );
        assert_eq!(f.physical().await, before);
        gate.release();
        f.idle().await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while f.store.write_turns.requests() != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("write turn released after actual rollback");
        assert_eq!(f.physical().await, before);
        assert!(f
            .store
            .replay("demo", "cpu-cancelled", "customer.create", "", &body)
            .await
            .expect("original receipt lookup")
            .is_none());
        let retained = f
            .store
            .mutate("demo", "cpu-original", "customer.create", "", original_body)
            .await
            .expect("existing original replays");
        assert_eq!(retained["replayed"], true);
        assert_eq!(retained["data"], original["data"]);
        assert_eq!(f.physical().await, before);
        let applied = f
            .store
            .mutate("demo", "cpu-cancelled", "customer.create", "", body.clone())
            .await
            .expect("exact original explicitly retried");
        assert_eq!(applied["replayed"], false);
        let committed = f.physical().await;
        let replay = f
            .store
            .mutate("demo", "cpu-cancelled", "customer.create", "", body)
            .await
            .expect("exact original replay");
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["data"], applied["data"]);
        assert_eq!(f.physical().await, committed);
        let workspace = f.store.read("demo").await.expect("retained source");
        assert_eq!(
            workspace
                .customers
                .iter()
                .filter(|customer| customer.name == "Exactly one explicit retry")
                .count(),
            1
        );
        f.clean().await;
    });
}
impl Fixture {
    fn budget() -> DatabaseBudget {
        DatabaseBudget {
            acquire_ms: 250,
            statement_ms: 1000,
            lock_ms: 200,
        }
    }
    async fn new() -> Self {
        Self::new_with_workspace(None).await
    }
    async fn new_with_workspace(jobs: Option<usize>) -> Self {
        Self::new_with_workspace_budget(jobs, Self::budget()).await
    }
    async fn new_with_workspace_budget(jobs: Option<usize>, budget: DatabaseBudget) -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("disposable database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("database_budget_{}", uuid::Uuid::new_v4().simple());
        let app = format!("database-budget-{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("url");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"))
            .append_pair("application_name", &app);
        let url = url.to_string();
        let mut store = Store::connect(Some(&url), budget).await.expect("store");
        store.read_slots = Arc::new(Semaphore::new(1));
        store.seed_demo().await.expect("seed");
        store.workspace_admission = jobs.map(|jobs| {
            Arc::new(WorkspaceAdmission::new(
                jobs,
                crate::read_admission::ReadLimits {
                    pending: 1,
                    tenant_pending: 1,
                    ..Default::default()
                },
            ))
        });
        let observer = PgPool::connect_with(
            url.parse::<sqlx::postgres::PgConnectOptions>()
                .expect("observer options")
                .application_name("owned-budget-observer"),
        )
        .await
        .expect("independent observer");
        let router = api::router(App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store: store.clone(),
            demo: false,
            integrations: Arc::new(Integrations::default()),
            keys: Arc::new(BTreeMap::from([("owned-budget".into(), "demo".into())])),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("HTTP");
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("client");
        Self {
            admin,
            observer,
            store,
            url,
            schema,
            app,
            origin,
            server,
            client,
        }
    }
    async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{path}", self.origin))
            .bearer_auth("owned-budget")
            .send()
            .await
            .expect("bounded HTTP")
    }
    async fn busy(response: reqwest::Response) {
        assert_eq!(response.status(), 503);
        assert_eq!(response.headers()["retry-after"], "1");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.json::<Value>().await.expect("JSON"),
            json!({"error":{"code":"database_temporarily_unavailable","message":DATABASE_BUSY}})
        );
    }
    async fn physical(&self) -> (Value, String, String, String) {
        sqlx::query_as("SELECT freight_workspace_originals(tenant_id),updated_at::text,xmin::text,ctid::text FROM freight_workspaces WHERE tenant_id='demo'")
            .fetch_one(&self.observer).await.expect("physical row")
    }
    async fn idle(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let active:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND state <> 'idle'")
                .bind(&self.app).fetch_one(&self.admin).await.expect("activity");
            if active == 0 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "connection did not roll back/return"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn clean(self) {
        self.server.abort();
        let _ = self.server.await;
        self.store.pool.as_ref().expect("pool").close().await;
        self.observer.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .expect("cleanup");
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_terminated_read_returns_unavailable_and_reconnects_without_changing_original_receipts(
) {
    let f = Fixture::new().await;
    let body = json!({"name":"Original before connection loss","contact":"owned@example.test"});
    let created: Value = f
        .client
        .post(format!("{}/v1/customers", f.origin))
        .bearer_auth("owned-budget")
        .header("Idempotency-Key", "availability-original")
        .json(&body)
        .send()
        .await
        .expect("original HTTP")
        .json()
        .await
        .expect("original JSON");
    assert_eq!(created["replayed"], false);
    let before = f.physical().await;
    let pool = f.store.pool.as_ref().expect("pool");
    let mut leases = Vec::new();
    for _ in 0..pool.options().get_max_connections() {
        let mut connection = pool.acquire().await.expect("owned connection");
        for statement in ["SET statement_timeout='5s'", "SET lock_timeout='5s'"] {
            sqlx::query(statement)
                .execute(&mut *connection)
                .await
                .expect("fault observation window");
        }
        leases.push(connection);
    }
    drop(leases);
    let mut lock = f.observer.begin().await.expect("owned blocker");
    sqlx::query("LOCK freight_workspaces IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .expect("block only this schema");
    let client = f.client.clone();
    let origin = f.origin.clone();
    let read = tokio::spawn(async move {
        client
            .get(format!("{origin}/v1/workspace"))
            .bearer_auth("owned-budget")
            .send()
            .await
            .expect("HTTP survives database termination")
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let pid: i32 = loop {
        let waiting:Option<i32>=sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND query LIKE '%freight_workspaces%' LIMIT 1")
            .bind(&f.app).fetch_optional(&f.admin).await.expect("own blocked backend");
        if let Some(pid) = waiting {
            break pid;
        }
        assert!(tokio::time::Instant::now() < deadline, "read never blocked");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(&f.admin)
        .await
        .expect("terminate exact owned read");
    assert!(terminated);
    let response = read.await.expect("read task");
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], "1");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.expect("error JSON"),
        json!({"error":{"code":"database_temporarily_unavailable","message":DATABASE_UNAVAILABLE}})
    );
    lock.rollback().await.expect("release blocker");
    assert_eq!(f.get("/v1/workspace").await.status(), 200);
    let replay: Value = f
        .client
        .post(format!("{}/v1/customers", f.origin))
        .bearer_auth("owned-budget")
        .header("Idempotency-Key", "availability-original")
        .json(&body)
        .send()
        .await
        .expect("replay HTTP")
        .json()
        .await
        .expect("replay JSON");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], created["data"]);
    assert_eq!(f.physical().await, before);
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_runtime_budgets_release_cancelled_reads_and_bound_pool_waits_and_statements() {
    let f = Fixture::new().await;
    let pool = f.store.pool.as_ref().expect("pool");
    let mut leases = Vec::new();
    for _ in 0..pool.options().get_max_connections() {
        let mut connection = pool.acquire().await.expect("lease");
        let settings: (String, String) = sqlx::query_as(
            "SELECT current_setting('statement_timeout'),current_setting('lock_timeout')",
        )
        .fetch_one(&mut *connection)
        .await
        .expect("runtime settings after migrations");
        assert_eq!(settings, ("1s".into(), "200ms".into()));
        leases.push(connection);
    }
    Fixture::busy(f.get("/v1/workspace").await).await;
    assert_eq!(f.get("/health").await.status(), 503);
    for connection in leases {
        connection.close().await.expect("close old backend");
    }
    let settings: (String, String) = sqlx::query_as(
        "SELECT current_setting('statement_timeout'),current_setting('lock_timeout')",
    )
    .fetch_one(pool)
    .await
    .expect("replacement backend");
    assert_eq!(settings, ("1s".into(), "200ms".into()));
    assert_eq!(f.get("/health").await.status(), 200);
    let mut blocker = f.observer.begin().await.expect("blocker");
    sqlx::query("LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("table lock");
    let reader = f.store.clone();
    let reading = tokio::spawn(async move { reader.read("demo").await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let waiting:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND query LIKE 'SELECT data::text%'")
            .bind(&f.app).fetch_one(&f.admin).await.expect("waiting read");
        if waiting > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "read did not reach real SQL"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    reading.abort();
    let _ = reading.await;
    assert_eq!(
        f.store.read_slots.available_permits(),
        0,
        "caller cancellation retains the running query's slot"
    );
    f.idle().await;
    assert_eq!(
        f.store.read_slots.available_permits(),
        1,
        "server timeout releases abandoned work without unlocking the table"
    );
    Fixture::busy(f.get("/v1/workspace").await).await;
    assert_eq!(f.get("/health").await.status(), 200);
    blocker.rollback().await.expect("unlock");
    assert_eq!(f.get("/v1/workspace").await.status(), 200);
    let error = sqlx::query("SELECT pg_sleep(5)")
        .execute(pool)
        .await
        .expect_err("statement must expire");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("57014")
    );
    assert_eq!(database_error(error), DATABASE_BUSY);
    f.idle().await;
    let error = sqlx::query("SELECT 1 / 0")
        .execute(pool)
        .await
        .expect_err("ordinary storage error");
    assert_eq!(database_error(error), "Internal storage error");
    assert_eq!(f.get("/v1/workspace").await.status(), 200);
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_runtime_timeouts_rollback_staged_files_and_receipts_before_exact_http_retry() {
    let f = Fixture::new().await;
    sqlx::raw_sql("CREATE SEQUENCE staged_blob_count; CREATE FUNCTION observe_blob() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('staged_blob_count'); RETURN NEW; END $$; CREATE TRIGGER observe_blob AFTER INSERT ON freight_document_content FOR EACH ROW EXECUTE PROCEDURE observe_blob();")
        .execute(&f.observer).await.expect("nontransactional staging witness");
    let bytes = b"%PDF-1.4\nowned runtime budget original bytes\n%%EOF\n";
    let mut saved = Vec::new();
    for (index, stall) in [
        "PERFORM pg_advisory_xact_lock(485197204);",
        "PERFORM pg_sleep(5);",
    ]
    .into_iter()
    .enumerate()
    {
        let mut blocker = f.observer.begin().await.expect("blocker");
        sqlx::query("SELECT pg_advisory_xact_lock(485197204)")
            .execute(&mut *blocker)
            .await
            .expect("advisory blocker");
        sqlx::raw_sql(&format!("CREATE OR REPLACE FUNCTION stall_workspace() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN {stall} RETURN NEW; END $$; DROP TRIGGER IF EXISTS stall_workspace ON freight_workspaces; CREATE TRIGGER stall_workspace BEFORE UPDATE ON freight_workspaces FOR EACH ROW EXECUTE PROCEDURE stall_workspace();"))
            .execute(&f.observer).await.expect("stall only owned tenant writes");
        let before = f.physical().await;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_document_content")
            .fetch_one(&f.observer)
            .await
            .expect("file count");
        let key = format!("runtime-budget-upload-{index}");
        let upload = || {
            f.client
                .post(format!(
                    "{}/v1/loads/load_1/documents?kind=other&filename=budget.pdf",
                    f.origin
                ))
                .bearer_auth("owned-budget")
                .header("Idempotency-Key", &key)
                .header("Content-Type", "application/pdf")
                .body(bytes.to_vec())
        };
        Fixture::busy(upload().send().await.expect("timed HTTP upload")).await;
        f.idle().await;
        assert_eq!(
            f.physical().await,
            before,
            "failed UPDATE retains complete physical row, receipts and journals"
        );
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_document_content")
            .fetch_one(&f.observer)
            .await
            .expect("files");
        assert_eq!(after, count, "staged original file rolls back too");
        let staged: (i64, bool) =
            sqlx::query_as("SELECT last_value,is_called FROM staged_blob_count")
                .fetch_one(&f.observer)
                .await
                .expect("staging witness");
        assert_eq!(
            staged,
            ((index as i64) * 2 + 1, true),
            "file INSERT actually preceded the rejected workspace UPDATE"
        );
        assert!(!before.0["receipts"]
            .as_object()
            .expect("receipts")
            .contains_key(&key));
        blocker.rollback().await.expect("unlock");
        sqlx::query("DROP TRIGGER stall_workspace ON freight_workspaces")
            .execute(&f.observer)
            .await
            .expect("resume writes");
        let response = upload().send().await.expect("same-key retry");
        assert_eq!(response.status(), 200);
        let result: Value = response.json().await.expect("result");
        assert_eq!(result["replayed"], false);
        let id = result["data"]["id"].as_str().expect("id").to_string();
        let replay = upload().send().await.expect("exact replay");
        assert_eq!(replay.status(), 200);
        let replay: Value = replay.json().await.expect("replay");
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["data"], result["data"]);
        let download = f.get(&format!("/v1/documents/{id}/content")).await;
        assert_eq!(download.status(), 200);
        assert_eq!(download.bytes().await.expect("bytes").as_ref(), bytes);
        saved.push((key, id));
    }
    let restarted = Store::connect(Some(&f.url), Fixture::budget())
        .await
        .expect("restart");
    let workspace = restarted.read("demo").await.expect("durable workspace");
    for (key, id) in saved {
        assert_eq!(workspace.receipts[&key].response["id"], id);
        assert_eq!(
            restarted
                .document_content("demo", &id)
                .await
                .expect("durable original")
                .1,
            bytes
        );
    }
    let staged: i64 = sqlx::query_scalar("SELECT last_value FROM staged_blob_count")
        .fetch_one(&f.observer)
        .await
        .expect("total attempts");
    assert_eq!(
        staged, 4,
        "two rejected attempts, two committed inserts, and no replay inserts"
    );
    restarted.pool.as_ref().expect("pool").close().await;
    f.clean().await;
}

#[path = "workspace_admission_postgres_tests.rs"]
mod workspace_admission_postgres_tests;

#[path = "catalog_admission_postgres_tests.rs"]
mod catalog_admission_postgres_tests;

#[path = "derived_index_admission_postgres_tests.rs"]
mod derived_index_admission_postgres_tests;

#[path = "deadline_admission_postgres_tests.rs"]
mod deadline_admission_postgres_tests;

#[path = "admission_diagnostics_postgres_tests.rs"]
mod admission_diagnostics_postgres_tests;
