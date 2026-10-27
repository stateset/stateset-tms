use super::*;
use crate::{domain, integrations::Connection};
use axum::{extract::Path, routing::get, Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Mutex, Semaphore};

struct Partner {
    url: String,
    calls: Arc<Mutex<Vec<String>>>,
    release: Arc<Semaphore>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Partner {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Partner {
    async fn new(block_all: bool) -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(Semaphore::new(0));
        let app = Router::new().route("/v1/documents/{document}", get({
            let calls = calls.clone(); let release = release.clone();
            move |Path(document): Path<String>| {
                let calls = calls.clone(); let release = release.clone();
                async move {
                    calls.lock().await.push(document.clone());
                    if block_all || document.starts_with("slow") {
                        release.acquire().await.expect("release").forget();
                    }
                    Json(json!({"data":{"document":{"id":document,"partner_id":"atfl","transaction_set":"990","direction":"inbound","status":"accepted","processed_at":Utc::now()},"parsed":{"shipment_id":"FR-1050","carrier_scac":"ATFL","action_code":"A"}}}))
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        Self {
            url,
            calls,
            release,
            server,
        }
    }
    fn integrations(&self, tenants: &[&str]) -> Integrations {
        Integrations {
            tenants: tenants
                .iter()
                .map(|tenant| {
                    (
                        (*tenant).into(),
                        Connection {
                            edi_url: Some(self.url.clone()),
                            edi_token: Some("owned-worker-key".into()),
                            edi_tenant_id: Some("owned-worker-tenant".into()),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        }
    }
}

async fn queued(store: &Store, tenant: &str, document: &str) {
    store
        .update(tenant, |w| {
            *w = domain::demo();
            Ok(Value::Null)
        })
        .await
        .expect("owned tenant");
    let note = json!({"type":"document.recorded","tenant_id":"owned-worker-tenant","document_id":document,"partner_id":"atfl","transaction_set":"990","direction":"inbound","status":"accepted"});
    store
        .receive_document(
            tenant,
            &format!("delivery-{document}"),
            "1",
            note.clone(),
            note.to_string(),
            None,
        )
        .await
        .expect("retained callback");
}
async fn wait_calls(partner: &Partner, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if partner.calls.lock().await.len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual upstream requests");
}
async fn wait_applied(store: &Store, tenant: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store.read(tenant).await.expect("read").inbox[0].status == "applied" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual inbox completion");
}

#[test]
fn worker_concurrency_rejects_invalid_configuration() {
    assert_eq!(WorkerConfig::parse(None).expect("default").concurrency, 2);
    for raw in ["1", "2", "16"] {
        assert_eq!(
            WorkerConfig::parse(Some(raw))
                .expect("valid")
                .concurrency
                .to_string(),
            raw
        );
    }
    for raw in [
        "",
        "0",
        "17",
        "-1",
        "+2",
        " 2",
        "2 ",
        "1.0",
        "999999999999999999999999999",
    ] {
        assert!(WorkerConfig::parse(Some(raw)).is_err(), "{raw}");
    }
}

async fn healthy_progress(store: &Store) {
    let partner = Partner::new(false).await;
    let tenants = ["a_slow", "b_ready", "c_ready"];
    for (tenant, doc) in tenants.iter().zip(["slow-990", "ready-b", "ready-c"]) {
        queued(store, tenant, doc).await;
    }
    let integrations = partner.integrations(&tenants);
    let task = tokio::spawn(worker(store.clone(), integrations, WorkerConfig::default()));
    wait_calls(&partner, 1).await;
    wait_applied(store, "b_ready").await;
    wait_applied(store, "c_ready").await;
    assert_eq!(
        store.read("a_slow").await.expect("slow original").inbox[0].status,
        "processing"
    );
    // More than one normal poll elapses. The blocked tenant must neither
    // overlap itself nor be required to finish before ready tenants rotate.
    tokio::time::sleep(POLL + Duration::from_millis(100)).await;
    let calls = partner.calls.lock().await.clone();
    assert_eq!(calls.iter().filter(|v| v.as_str() == "slow-990").count(), 1);
    assert_eq!(calls.len(), 3);
    assert!(!task.is_finished());
    partner.release.add_permits(1);
    wait_applied(store, "a_slow").await;
    let slow = store.read("a_slow").await.expect("retained original");
    assert_eq!(slow.inbox[0].total_attempts, 1);
    assert_eq!(slow.inbox[0].delivery_ids, vec!["delivery-slow-990"]);
    assert_eq!(slow.loads[2].status, Status::Accepted);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn held_partner_does_not_block_ready_tenants_or_overlap_its_own_cycle() {
    healthy_progress(&Store::new(None).await.expect("memory")).await;
}

#[tokio::test]
async fn full_worker_slots_bound_claims_rotate_waiters_and_cancel_with_the_owner() {
    let store = Store::new(None).await.expect("memory");
    let partner = Partner::new(true).await;
    let tenants = ["a", "b", "c", "d", "e"];
    for tenant in tenants {
        queued(&store, tenant, tenant).await;
    }
    let task = tokio::spawn(worker(
        store.clone(),
        partner.integrations(&tenants),
        WorkerConfig::default(),
    ));
    wait_calls(&partner, 2).await;
    tokio::time::sleep(POLL + Duration::from_millis(100)).await;
    assert_eq!(
        partner.calls.lock().await.len(),
        2,
        "only two tenant cycles may reach the partner"
    );
    for tenant in ["c", "d", "e"] {
        let w = store.read(tenant).await.expect("waiting tenant");
        assert_eq!(w.inbox[0].status, "pending");
        assert_eq!(w.inbox[0].total_attempts, 0);
    }
    partner.release.add_permits(2);
    wait_calls(&partner, 4).await;
    let calls = partner.calls.lock().await.clone();
    assert_eq!(
        calls[..2]
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["a".to_string(), "b".to_string()].into()
    );
    assert_eq!(
        calls[2..4]
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["c".to_string(), "d".to_string()].into()
    );
    let original = store.read("c").await.expect("owned lease").inbox[0].clone();
    assert_eq!(original.status, "processing");
    task.abort();
    assert!(task.await.expect_err("owner cancelled").is_cancelled());
    partner.release.add_permits(20);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let retained = store
        .read("c")
        .await
        .expect("retained interrupted lease")
        .inbox[0]
        .clone();
    assert_eq!(retained.lease, original.lease);
    assert_eq!(
        retained.status, "processing",
        "cancelled owner cannot complete a child request"
    );
    assert_eq!(retained.total_attempts, 1);
    assert_eq!(
        partner.calls.lock().await.len(),
        4,
        "queued fifth tenant was never launched after cancellation"
    );
}

#[tokio::test]
async fn graceful_drain_finishes_original_inbound_but_never_claims_waiting_tenants_or_outbound() {
    let store = Store::new(None).await.expect("memory");
    let partner = Partner::new(true).await;
    for tenant in ["a", "b"] {
        queued(&store, tenant, tenant).await;
    }
    store
        .mutate(
            "a",
            "original-tender",
            "load.command",
            "load_5",
            json!({"action":"tender","carrier_id":"ca2","expected_version":1}),
        )
        .await
        .expect("real queued tender");
    let original = store.read("a").await.expect("original workspace").outbox;
    assert!(!original.is_empty());
    let shutdown = Shutdown::default();
    let task = tokio::spawn(worker_until(
        store.clone(),
        partner.integrations(&["a", "b"]),
        WorkerConfig::parse(Some("1")).expect("one slot"),
        shutdown.clone(),
    ));
    wait_calls(&partner, 1).await;
    shutdown.request();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !task.is_finished(),
        "current response and its guarded commit must finish"
    );
    assert_eq!(
        store.read("a").await.expect("active lease").inbox[0].status,
        "processing"
    );
    partner.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("drain completes without another poll")
        .expect("worker");
    let a = store.read("a").await.expect("finished original");
    assert_eq!(a.inbox[0].status, "applied");
    assert_eq!(a.inbox[0].total_attempts, 1);
    assert_eq!(a.inbox[0].delivery_ids, ["delivery-a"]);
    assert_eq!(a.loads[2].status, Status::Accepted);
    assert_eq!(
        json!(a.outbox),
        json!(original),
        "no outbound claim after the admitted inbound completed"
    );
    let b = store.read("b").await.expect("unstarted tenant");
    assert_eq!(b.inbox[0].status, "pending");
    assert_eq!(b.inbox[0].total_attempts, 0);
    assert_eq!(partner.calls.lock().await.as_slice(), ["a"]);
}

#[tokio::test]
async fn shutdown_before_worker_start_never_claims_or_runs_periodic_work() {
    let store = Store::new(None).await.expect("memory");
    let partner = Partner::new(true).await;
    queued(&store, "a", "a").await;
    let original = store.read("a").await.expect("original");
    let shutdown = Shutdown::default();
    shutdown.request();
    tokio::time::timeout(
        Duration::from_secs(1),
        worker_until(
            store.clone(),
            partner.integrations(&["a"]),
            WorkerConfig::default(),
            shutdown.clone(),
        ),
    )
    .await
    .expect("integration worker stops");
    tokio::time::timeout(
        Duration::from_secs(1),
        crate::operations::worker_until(store.clone(), shutdown),
    )
    .await
    .expect("operations worker stops");
    assert_eq!(
        json!(store.read("a").await.expect("unchanged")),
        json!(original)
    );
    assert!(partner.calls.lock().await.is_empty());
}

#[tokio::test]
async fn expired_unconfigured_tender_is_visible_while_the_connector_is_held() {
    let store = Store::new(None).await.expect("memory");
    let partner = Partner::new(true).await;
    queued(&store, "a_slow", "slow").await;
    store
        .update("unconfigured", |w| {
            *w = domain::demo();
            Ok(Value::Null)
        })
        .await
        .expect("original workspace");
    store
        .mutate(
            "unconfigured",
            "original-tender",
            "load.command",
            "load_5",
            json!({"action":"tender","expected_version":1,"carrier_id":"ca2"}),
        )
        .await
        .expect("actual tender command");
    let original = store
        .read("unconfigured")
        .await
        .expect("original tender")
        .tenders[0]
        .clone();
    store
        .update("unconfigured", |w| {
            w.loads[4].tender_expires_at = Some(Utc::now() - chrono::Duration::minutes(1));
            w.loads[4].exception = None;
            Ok(Value::Null)
        })
        .await
        .expect("expired original tender");
    let task = tokio::spawn(worker(
        store.clone(),
        partner.integrations(&["a_slow"]),
        WorkerConfig::parse(Some("1")).expect("one slot"),
    ));
    wait_calls(&partner, 1).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let w = store.read("unconfigured").await.expect("expiry");
            if let Some(note) = &w.loads[4].exception {
                assert!(note.contains("Tender response overdue"));
                assert_eq!(w.loads[4].status, Status::Tendered);
                assert_eq!(
                    json!(w.tenders[0]),
                    json!(original),
                    "expiry never retenders or replaces the original agreement"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expiry is independent of the blocked network call");
    assert_eq!(
        store.read("a_slow").await.expect("held").inbox[0].status,
        "processing"
    );
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn panicking_cycle_returns_its_tenant_to_the_queue_with_backoff() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let first = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(schedule(
        vec!["a".into(), "b".into(), "c".into()],
        WorkerConfig::parse(Some("1")).expect("one slot"),
        Shutdown::default(),
        {
            let calls = calls.clone();
            let first = first.clone();
            move |mut state| {
                let calls = calls.clone();
                let first = first.clone();
                async move {
                    calls
                        .lock()
                        .await
                        .push((state.tenant.clone(), Instant::now()));
                    if state.tenant == "a" && first.fetch_add(1, Ordering::SeqCst) == 0 {
                        panic!("owned injected cycle panic");
                    }
                    state.source_cursor += 1;
                    state
                }
            }
        },
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if calls.lock().await.len() >= 4 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("panicked tenant was not lost");
    let calls = calls.lock().await.clone();
    assert_eq!(
        calls[..4].iter().map(|v| v.0.as_str()).collect::<Vec<_>>(),
        ["a", "b", "c", "a"]
    );
    assert!(
        calls[3].1 - calls[0].1 >= POLL,
        "panic recovery must not spin"
    );
    task.abort();
    let _ = task.await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_workers_keep_healthy_tenants_moving_and_preserve_original_completions_after_reconnect(
) {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("tenant_worker_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let store = Store::new(Some(url.as_str())).await.expect("store");
    healthy_progress(&store).await;
    drop(store);
    let restarted = Store::new(Some(url.as_str())).await.expect("reconnect");
    for tenant in ["a_slow", "b_ready", "c_ready"] {
        let w = restarted.read(tenant).await.expect("retained tenant");
        assert_eq!(w.inbox.len(), 1);
        assert_eq!(w.inbox[0].status, "applied");
        assert_eq!(w.inbox[0].total_attempts, 1);
        assert_eq!(w.loads[2].status, Status::Accepted);
    }
    drop(restarted);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("cleanup");
    admin.close().await;
}
