use super::*;
use crate::{api, integrations::Integrations, write_admission::WriteLimits};
use std::time::Duration;

struct Fixture {
    admin: PgPool,
    observer: PgPool,
    store: Store,
    schema: String,
    app: String,
    origin: String,
    server: tokio::task::JoinHandle<()>,
    client: reqwest::Client,
    original: Value,
}
fn customer(name: &str) -> Value {
    json!({"name":name,"contact":"owned@example.test"})
}
impl Fixture {
    async fn new() -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("write_contention_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let application_name = schema.clone();
        let mut url = reqwest::Url::parse(&base).expect("url");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema},pg_catalog"))
            .append_pair("application_name", &application_name);
        let observer = PgPool::connect(url.as_str()).await.expect("observer");
        let mut store = Store::new(Some(url.as_str())).await.expect("store");
        store.write_turns = Arc::new(WriteTurns::new(
            WriteLimits {
                total: 3,
                tenant: 2,
            },
            Duration::from_secs(5),
        ));
        let original = store
            .mutate(
                "hot",
                "owned-original",
                "customer.create",
                "",
                customer("Original"),
            )
            .await
            .expect("original");
        store
            .mutate(
                "cold",
                "owned-cold",
                "customer.create",
                "",
                customer("Cold original"),
            )
            .await
            .expect("cold");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let app = api::router(api::App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store: store.clone(),
            demo: false,
            integrations: Arc::new(Integrations::default()),
            keys: Arc::new(BTreeMap::from([
                ("owned-hot".into(), "hot".into()),
                ("owned-cold".into(), "cold".into()),
            ])),
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server");
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");
        Self {
            admin,
            observer,
            store,
            schema,
            app: application_name,
            origin,
            server,
            client,
            original,
        }
    }
    async fn physical(&self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('header',(SELECT jsonb_build_array(data::text,updated_at::text,xmin::text,ctid::text) FROM freight_workspaces WHERE tenant_id='hot'),'blocks',(SELECT jsonb_agg(jsonb_build_array(root,key,payload::text,xmin::text,ctid::text) ORDER BY root,key) FROM freight_workspace_blocks WHERE tenant_id='hot'))")
            .fetch_one(&self.observer).await.expect("physical originals")
    }
    async fn wait_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.store.write_turns.requests() != count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("actual admission count");
    }
    async fn wait_sql(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if self.sql_waiters().await == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("actual database lock waiter");
    }
    async fn sql_waiters(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock'")
            .bind(&self.app).fetch_one(&self.admin).await.expect("waiters")
    }
    fn save(&self, key: &str, name: &str) -> tokio::task::JoinHandle<Result<Value, String>> {
        let store = self.store.clone();
        let key = key.to_owned();
        let body = customer(name);
        tokio::spawn(async move { store.mutate("hot", &key, "customer.create", "", body).await })
    }
    async fn post(&self, key: &str, name: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/customers", self.origin))
            .bearer_auth("owned-hot")
            .header("Idempotency-Key", key)
            .json(&customer(name))
            .send()
            .await
            .expect("bounded post")
    }
    async fn healthy_other_tenant(&self) {
        for (path, token) in [
            ("/v1/workspace", "owned-cold"),
            ("/ready", "owned-cold"),
            ("/live", "owned-cold"),
        ] {
            let response = tokio::time::timeout(
                Duration::from_millis(750),
                self.client
                    .get(format!("{}{path}", self.origin))
                    .bearer_auth(token)
                    .send(),
            )
            .await
            .expect("hot waiters cannot occupy both connections")
            .expect("HTTP probe");
            assert_eq!(response.status(), 200);
        }
    }
    async fn original_replays(&self) {
        let response = tokio::time::timeout(
            Duration::from_millis(750),
            self.post("owned-original", "Original"),
        )
        .await
        .expect("original bypasses admission and tenant lock");
        assert_eq!(response.status(), 200);
        let response: Value = response.json().await.expect("original response");
        assert_eq!(response["data"], self.original["data"]);
        assert_eq!(response["replayed"], true);
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
async fn postgres_write_waiters_preserve_other_tenant_and_full_queue_original_recovery() {
    let f = Fixture::new().await;
    let before = f.physical().await;
    let mut blocker = f.observer.begin().await.expect("blocker");
    sqlx::query("SELECT tenant_id FROM freight_workspaces WHERE tenant_id='hot' FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("owned row lock");
    let first = f.save("owned-first", "First pending");
    f.wait_sql().await;
    let second = f.save("owned-second", "Second pending");
    f.wait_requests(2).await;
    assert_eq!(
        f.sql_waiters().await,
        1,
        "queued same-tenant writer uses no connection"
    );
    f.healthy_other_tenant().await;
    let read_capacity = f
        .store
        .read_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .expect("hold snapshot capacity");
    let full = f.post("owned-third", "Refused third").await;
    assert_eq!(full.status(), 503);
    assert_eq!(full.headers()["retry-after"], "1");
    assert_eq!(full.headers()["cache-control"], "no-store");
    assert_eq!(
        full.json::<Value>().await.expect("busy JSON"),
        json!({"error":{"code":"write_capacity_exceeded","message":WRITE_BUSY}})
    );
    f.original_replays().await;
    assert_eq!(f.physical().await, before);
    second.abort();
    assert!(second
        .await
        .expect_err("queued caller cancelled")
        .is_cancelled());
    f.wait_requests(1).await;
    assert_eq!(f.sql_waiters().await, 1);
    blocker.rollback().await.expect("release actual lock");
    assert_eq!(
        first.await.expect("first task").expect("first write")["replayed"],
        false
    );
    f.wait_requests(0).await;
    drop(read_capacity);
    let w = f.store.read("hot").await.expect("retained source");
    assert!(w.receipts.contains_key("owned-first"));
    assert!(!w.receipts.contains_key("owned-second") && !w.receipts.contains_key("owned-third"));
    assert_eq!(
        w.customers
            .iter()
            .filter(|c| c.name == "First pending")
            .count(),
        1
    );
    let applied = f
        .store
        .mutate(
            "hot",
            "owned-second",
            "customer.create",
            "",
            customer("Second pending"),
        )
        .await
        .expect("exact explicit retry");
    assert_eq!(applied["replayed"], false);
    let committed = f.physical().await;
    let replay = f
        .store
        .mutate(
            "hot",
            "owned-second",
            "customer.create",
            "",
            customer("Second pending"),
        )
        .await
        .expect("original replay");
    assert_eq!(replay["data"], applied["data"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(f.physical().await, committed);
    f.original_replays().await;
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_cancelled_sql_retains_write_turn_until_actual_rollback_finishes() {
    let f = Fixture::new().await;
    let before = f.physical().await;
    let mut blocker = f.observer.begin().await.expect("blocker");
    sqlx::query("SELECT tenant_id FROM freight_workspaces WHERE tenant_id='hot' FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("actual row lock");
    let cancelled = f.save("owned-cancelled-sql", "Cancelled SQL");
    f.wait_sql().await;
    cancelled.abort();
    assert!(cancelled
        .await
        .expect_err("cancelled SQL caller")
        .is_cancelled());
    f.wait_requests(1).await;
    let queued = f.save("owned-after-cancel", "Queued after cancellation");
    f.wait_requests(2).await;
    assert_eq!(
        f.sql_waiters().await,
        1,
        "rollback still owns the first turn; next write cannot occupy another connection"
    );
    f.healthy_other_tenant().await;
    f.original_replays().await;
    assert_eq!(f.physical().await, before);
    queued.abort();
    assert!(queued
        .await
        .expect_err("queued caller cancelled")
        .is_cancelled());
    f.wait_requests(1).await;
    blocker.rollback().await.expect("release owned row lock");
    f.wait_requests(0).await;
    assert_eq!(f.physical().await, before);
    let w = f.store.read("hot").await.expect("originals");
    assert!(
        !w.receipts.contains_key("owned-cancelled-sql")
            && !w.receipts.contains_key("owned-after-cancel")
    );
    let applied = f
        .store
        .mutate(
            "hot",
            "owned-cancelled-sql",
            "customer.create",
            "",
            customer("Cancelled SQL"),
        )
        .await
        .expect("exact retry");
    assert_eq!(applied["replayed"], false);
    let committed = f.physical().await;
    let replay = f
        .store
        .mutate(
            "hot",
            "owned-cancelled-sql",
            "customer.create",
            "",
            customer("Cancelled SQL"),
        )
        .await
        .expect("replay");
    assert_eq!(replay["data"], applied["data"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(f.physical().await, committed);
    f.original_replays().await;
    f.clean().await;
}
