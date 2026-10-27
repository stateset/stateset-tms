use super::*;
use crate::{domain, lifecycle::Shutdown};
use axum::{
    body::Bytes,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{Mutex, Semaphore};

type WireRequests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

struct Peer {
    integrations: Integrations,
    reads: Arc<Mutex<Vec<String>>>,
    sends: WireRequests,
    unavailable: Arc<AtomicBool>,
    hold_source: Arc<AtomicBool>,
    hold_send: Arc<AtomicBool>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn new(orders: BTreeMap<String, Value>) -> Self {
        let orders = Arc::new(orders);
        let reads = Arc::new(Mutex::new(Vec::new()));
        let sends = Arc::new(Mutex::new(Vec::new()));
        let unavailable = Arc::new(AtomicBool::new(true));
        let hold_source = Arc::new(AtomicBool::new(false));
        let hold_send = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let router = Router::new()
            .route(
                "/v1/tenants/owned-source/orders/{id}",
                get({
                    let (orders, reads, unavailable, hold, entered, release) = (
                        orders.clone(),
                        reads.clone(),
                        unavailable.clone(),
                        hold_source.clone(),
                        entered.clone(),
                        release.clone(),
                    );
                    move |Path(id): Path<String>, headers: HeaderMap| {
                        let (orders, reads, unavailable, hold, entered, release) = (
                            orders.clone(),
                            reads.clone(),
                            unavailable.clone(),
                            hold.clone(),
                            entered.clone(),
                            release.clone(),
                        );
                        async move {
                            assert_eq!(headers["x-stateset-api-key"], "owned-source-key");
                            reads.lock().await.push(id.clone());
                            if hold.swap(false, Ordering::SeqCst) {
                                entered.add_permits(1);
                                release.acquire().await.expect("held source").forget();
                            }
                            if unavailable.load(Ordering::SeqCst) {
                                return StatusCode::SERVICE_UNAVAILABLE.into_response();
                            }
                            Json(json!({"data":orders.get(&id).expect("original order")}))
                                .into_response()
                        }
                    }
                }),
            )
            .route(
                "/v1/edi/outbound/204/{partner}",
                post({
                    let (sends, hold, entered, release) = (
                        sends.clone(),
                        hold_send.clone(),
                        entered.clone(),
                        release.clone(),
                    );
                    move |headers: HeaderMap, body: Bytes| {
                        let (sends, hold, entered, release) = (
                            sends.clone(),
                            hold.clone(),
                            entered.clone(),
                            release.clone(),
                        );
                        async move {
                            assert_eq!(headers["authorization"], "Bearer owned-edi-key");
                            assert_eq!(headers["x-tenant-id"], "owned-edi-tenant");
                            sends.lock().await.push((
                                headers["idempotency-key"].to_str().expect("key").into(),
                                body.to_vec(),
                            ));
                            if hold.swap(false, Ordering::SeqCst) {
                                entered.add_permits(1);
                                release.acquire().await.expect("held handoff").forget();
                            }
                            "ISA*OWNED ORIGINAL ACKNOWLEDGEMENT"
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned peer");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let task = tokio::spawn(async move { axum::serve(listener, router).await.expect("peer") });
        let integrations = Integrations {
            tenants: BTreeMap::from([(
                "demo".into(),
                Connection {
                    edi_url: Some(url.clone()),
                    edi_token: Some("owned-edi-key".into()),
                    edi_tenant_id: Some("owned-edi-tenant".into()),
                    sync_url: Some(url),
                    sync_api_key: Some("owned-source-key".into()),
                    sync_tenant_id: Some("owned-source".into()),
                    ..Default::default()
                },
            )]),
        };
        Self {
            integrations,
            reads,
            sends,
            unavailable,
            hold_source,
            hold_send,
            entered,
            release,
            task,
        }
    }
    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .expect("actual held request")
            .expect("entered")
            .forget();
    }
}
fn plan(reference: &str) -> Value {
    let day = (Utc::now() + chrono::Duration::days(2)).date_naive();
    json!({"reference":reference,"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Original freight","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":day,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"},"buy_cents":123456,"sell_cents":150000})
}
async fn setup(store: &Store) -> (Peer, Vec<Outbox>) {
    store.seed_demo().await.expect("seed");
    let mut orders = BTreeMap::new();
    for index in 0..8 {
        let reference = format!("TURN-{index}");
        let load = if index < 6 {
            let id = uuid::Uuid::new_v4().to_string();
            let order = json!({"id":id,"orderNumber":format!("PO-{index}"),"status":"pending","updatedAt":"2026-10-01T00:00:00Z","lineItems":[{"sku":"ORIGINAL","quantity":1}]});
            orders.insert(id, order.clone());
            store
                .mutate(
                    "demo",
                    &format!("import-{index}"),
                    "sync.import",
                    "",
                    json!({"order":order,"load":plan(&reference)}),
                )
                .await
                .expect("real import")["data"]
                .clone()
        } else {
            store
                .mutate(
                    "demo",
                    &format!("create-{index}"),
                    "load.create",
                    "",
                    plan(&reference),
                )
                .await
                .expect("real local load")["data"]
                .clone()
        };
        store
            .mutate(
                "demo",
                &format!("tender-{index}"),
                "load.command",
                load["id"].as_str().expect("load id"),
                json!({"action":"tender","expected_version":load["version"],"carrier_id":"ca4"}),
            )
            .await
            .expect("real original tender");
    }
    let original = store.read("demo").await.expect("original").outbox;
    assert_eq!(original.len(), 8);
    (Peer::new(orders).await, original)
}
async fn bounded_progress(store: &Store) {
    let (peer, original) = setup(store).await;
    let mut cursor = DispatchCursor::default();
    let shutdown = Shutdown::default();
    assert!(!peer
        .integrations
        .dispatch_turn(store, "demo", &mut cursor, &shutdown)
        .await
        .expect("bounded turn"));
    assert_eq!(
        peer.reads.lock().await.len(),
        4,
        "only four candidate source checks per turn"
    );
    assert!(peer.sends.lock().await.is_empty());
    assert_eq!(cursor.0.as_deref(), Some(original[3].id.as_str()));
    assert!(peer
        .integrations
        .dispatch_turn(store, "demo", &mut cursor, &shutdown)
        .await
        .expect("later healthy load"));
    assert_eq!(peer.reads.lock().await.len(), 6);
    assert_eq!(peer.sends.lock().await[0].0, original[6].id);
    assert!(peer
        .integrations
        .dispatch_turn(store, "demo", &mut cursor, &shutdown)
        .await
        .expect("next healthy load"));
    let held = store.read("demo").await.expect("retained original jobs");
    for (job, old) in held.outbox[..6].iter().zip(&original) {
        assert_eq!(
            json!(job),
            json!(old),
            "a source outage never claims or rewrites its original unsent job"
        );
    }
    peer.unavailable.store(false, Ordering::SeqCst);
    for _ in 0..6 {
        assert!(peer
            .integrations
            .dispatch_turn(store, "demo", &mut cursor, &shutdown)
            .await
            .expect("wrapped original recovery"));
    }
    assert!(!peer
        .integrations
        .dispatch_turn(store, "demo", &mut cursor, &shutdown)
        .await
        .expect("fully drained queue"));
    let current = store.read("demo").await.expect("final");
    for (job, old) in current.outbox.iter().zip(&original) {
        assert_eq!(job.id, old.id);
        assert_eq!(job.payload, old.payload);
        assert_eq!(job.status, "gateway_accepted");
        assert_eq!(job.total_attempts, 1);
        let sends = peer.sends.lock().await;
        let matches: Vec<_> = sends.iter().filter(|(key, _)| key == &old.id).collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].1,
            serde_json::to_vec(&old.payload).expect("original payload bytes")
        );
    }
    assert!(current
        .loads
        .iter()
        .filter(|l| l.reference.starts_with("TURN-"))
        .all(|l| l.status == domain::Status::Tendered));
}
#[tokio::test]
async fn source_outage_backlog_is_bounded_and_rotates_to_healthy_jobs_then_recovers_original_requests(
) {
    bounded_progress(&Store::new(None).await.expect("memory")).await;
}
#[tokio::test]
async fn rotation_preserves_destination_ordering_and_bookmarks_survive_eligibility_changes() {
    let store = Store::new(None).await.expect("memory");
    let (peer, original) = setup(&store).await;
    let mut cursor = DispatchCursor::default();
    let shutdown = Shutdown::default();
    assert!(!peer
        .integrations
        .dispatch_turn(&store, "demo", &mut cursor, &shutdown)
        .await
        .expect("first turn"));
    store
        .update("demo", |w| {
            w.outbox[0].status = "held".into();
            w.outbox[1].status = "held".into();
            let mut dependent = w.outbox[2].clone();
            dependent.id = "owned-dependent-request".into();
            w.outbox.push(dependent);
            Ok(Value::Null)
        })
        .await
        .expect("changed eligibility and later dependent job");
    assert!(peer
        .integrations
        .dispatch_turn(&store, "demo", &mut cursor, &shutdown)
        .await
        .expect("bookmark retains next position"));
    assert_eq!(peer.sends.lock().await[0].0, original[6].id);
    cursor.0 = Some(original[7].id.clone());
    assert!(!peer
        .integrations
        .dispatch_turn(&store, "demo", &mut cursor, &shutdown)
        .await
        .expect("blocked dependent remains ordered"));
    assert_eq!(peer.sends.lock().await.len(), 1);
    assert_eq!(
        store
            .read("demo")
            .await
            .expect("dependent")
            .outbox
            .last()
            .expect("last")
            .total_attempts,
        0
    );
    cursor.0 = Some("missing-bookmark".into());
    assert!(!peer
        .integrations
        .dispatch_turn(&store, "demo", &mut cursor, &shutdown)
        .await
        .expect("missing bookmark starts at queue head"));
    assert!(peer
        .integrations
        .dispatch_turn(&store, "demo", &mut cursor, &shutdown)
        .await
        .expect("next turn still reaches healthy tail"));
    assert_eq!(peer.sends.lock().await[1].0, original[7].id);
}
#[tokio::test]
async fn shutdown_during_source_verification_never_starts_a_fresh_handoff_or_another_source_check()
{
    let store = Store::new(None).await.expect("memory");
    let (peer, original) = setup(&store).await;
    peer.unavailable.store(false, Ordering::SeqCst);
    peer.hold_source.store(true, Ordering::SeqCst);
    let shutdown = Shutdown::default();
    let task = tokio::spawn({
        let store = store.clone();
        let integrations = peer.integrations.clone();
        let shutdown = shutdown.clone();
        async move {
            integrations
                .dispatch_turn(&store, "demo", &mut DispatchCursor::default(), &shutdown)
                .await
        }
    });
    peer.entered().await;
    shutdown.request();
    peer.release.add_permits(1);
    assert!(!task.await.expect("task").expect("stopped before handoff"));
    assert_eq!(peer.reads.lock().await.len(), 1);
    assert!(peer.sends.lock().await.is_empty());
    assert_eq!(
        json!(store.read("demo").await.expect("original jobs").outbox),
        json!(original)
    );
}
#[tokio::test]
async fn an_already_claimed_handoff_finishes_during_drain_without_sending_the_next_job() {
    let store = Store::new(None).await.expect("memory");
    let (peer, original) = setup(&store).await;
    peer.hold_send.store(true, Ordering::SeqCst);
    let shutdown = Shutdown::default();
    let task = tokio::spawn({
        let store = store.clone();
        let integrations = peer.integrations.clone();
        let shutdown = shutdown.clone();
        let mut cursor = DispatchCursor(Some(original[5].id.clone()));
        async move {
            integrations
                .dispatch_turn(&store, "demo", &mut cursor, &shutdown)
                .await
        }
    });
    peer.entered().await;
    let claimed = store.read("demo").await.expect("claimed").outbox[6].clone();
    assert_eq!(claimed.status, "sending");
    assert!(claimed.lease.is_some());
    shutdown.request();
    peer.release.add_permits(1);
    assert!(task.await.expect("task").expect("finish claimed handoff"));
    let current = store.read("demo").await.expect("completed");
    assert_eq!(current.outbox[6].status, "gateway_accepted");
    assert_eq!(current.outbox[6].total_attempts, 1);
    assert_eq!(json!(current.outbox[7]), json!(original[7]));
    assert_eq!(peer.sends.lock().await.len(), 1);
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_bounded_dispatch_recovery_preserves_original_keys_payloads_and_completions_after_reconnect(
) {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("dispatch_turn_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let store = Store::new(Some(url.as_str())).await.expect("store");
    bounded_progress(&store).await;
    drop(store);
    let restarted = Store::new(Some(url.as_str())).await.expect("reconnect");
    let current = restarted.read("demo").await.expect("persisted");
    assert_eq!(current.outbox.len(), 8);
    assert!(current
        .outbox
        .iter()
        .all(|job| job.status == "gateway_accepted" && job.total_attempts == 1));
    drop(current);
    drop(restarted);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("cleanup");
    admin.close().await;
}
