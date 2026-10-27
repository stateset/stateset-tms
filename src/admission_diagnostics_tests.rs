use super::*;
use crate::{
    api::{self, App},
    auth::Credential,
    integrations::Integrations,
    lifecycle::Shutdown,
    observability::Observability,
};
use axum::{body::Body, http::Request, Router};
use http_body_util::BodyExt;
use std::time::Duration;
use tower::ServiceExt;

const TOKEN: &str = "diagnostic-monitor-only-synthetic-credential-123456";

pub(super) fn router(store: Store) -> Router {
    api::router_with_observability(
        App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store,
            keys: Arc::new(BTreeMap::from([(
                "separate-business-synthetic-credential-123456".into(),
                Credential::from("secret-business-tenant"),
            )])),
            demo: true,
            integrations: Arc::new(Integrations::default()),
        },
        Observability::new(Some(TOKEN), Shutdown::default()).expect("dedicated monitor"),
    )
}

pub(super) async fn scrape(router: &Router) -> String {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .expect("metrics body")
            .to_bytes()
            .to_vec(),
    )
    .expect("UTF8");
    assert!(!text.contains(TOKEN));
    assert!(!text.contains("secret-business-tenant"));
    text
}

#[tokio::test]
async fn metrics_show_real_snapshot_waiter_and_reclaim_only_cancelled_waiter() {
    let store = Store::new(None).await.expect("store");
    let app = router(store.clone());
    let held = store
        .read_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .expect("all read permits");
    let reader = store.clone();
    let waiting = tokio::spawn(async move { reader.read("secret-queued-tenant").await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.read_turns.pending_count() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("real read is queued");
    let text = scrape(&app).await;
    assert!(text.contains("freight_admission_pending{lane=\"snapshot\"} 1\n"));
    assert!(text.contains("freight_admission_available_permits{lane=\"snapshot\"} 0\n"));
    assert!(text.contains("freight_admission_pending_limit{lane=\"snapshot\"} 64\n"));
    assert!(text.contains("freight_admission_tenant_pending_limit{lane=\"snapshot\"} 16\n"));
    assert!(text.contains("freight_workspace_admission_configured 0\n"));
    assert!(text.contains("freight_database_pool_configured 0\n"));
    assert!(!text
        .lines()
        .any(|line| line.starts_with("freight_database_pool_connections ")));
    assert!(!text.contains("lane=\"workspace\""));
    assert!(!text.contains("secret-queued-tenant"));
    waiting.abort();
    assert!(waiting.await.expect_err("cancelled").is_cancelled());
    let text = scrape(&app).await;
    assert!(text.contains("freight_admission_pending{lane=\"snapshot\"} 0\n"));
    assert!(text.contains("freight_admission_available_permits{lane=\"snapshot\"} 0\n"));
    drop(held);
    assert!(scrape(&app)
        .await
        .contains("freight_admission_available_permits{lane=\"snapshot\"} 2\n"));
}

#[tokio::test]
async fn metrics_keep_shared_work_lease_until_last_actual_owner_drops() {
    let mut store = Store::new(None).await.expect("store");
    store.workspace_admission = Some(Arc::new(WorkspaceAdmission::new(
        1,
        crate::read_admission::ReadLimits::default(),
    )));
    let work = store
        .workspace_admission
        .as_ref()
        .expect("configured")
        .clone();
    let app = router(store.clone());
    let owner = work.acquire("secret-work-owner").await.expect("work");
    let retained = owner.clone();
    drop(owner);
    let next = work.clone();
    let waiter = tokio::spawn(async move { next.acquire("secret-work-waiter").await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while work.pending() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("queued work");
    let text = scrape(&app).await;
    assert!(text.contains("freight_workspace_admission_configured 1\n"));
    assert!(text.contains("freight_admission_pending{lane=\"workspace\"} 1\n"));
    assert!(text.contains("freight_admission_available_permits{lane=\"workspace\"} 0\n"));
    assert!(!text.contains("secret-work-"));
    waiter.abort();
    assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
    assert!(scrape(&app)
        .await
        .contains("freight_admission_available_permits{lane=\"workspace\"} 0\n"));
    drop(retained);
    assert!(scrape(&app)
        .await
        .contains("freight_admission_available_permits{lane=\"workspace\"} 1\n"));
}

#[tokio::test]
async fn metrics_count_write_owners_and_waiters_without_tenant_labels() {
    let store = Store::new(None).await.expect("store");
    let app = router(store.clone());
    let owner = store
        .write_turns
        .request("secret-writer")
        .expect("registered")
        .wait(store.write_turns.deadline())
        .await
        .expect("active");
    let admission = store
        .write_turns
        .request("secret-writer")
        .expect("registered waiter");
    let deadline = store.write_turns.deadline();
    let waiter = tokio::spawn(async move { admission.wait(deadline).await });
    let text = scrape(&app).await;
    assert!(text.contains("freight_write_admission_requests 2\n"));
    assert!(text.contains("freight_write_admission_limit 64\n"));
    assert!(text.contains("freight_write_admission_tenant_limit 16\n"));
    assert!(!text.contains("secret-writer"));
    waiter.abort();
    assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
    assert!(scrape(&app)
        .await
        .contains("freight_write_admission_requests 1\n"));
    drop(owner);
    assert!(scrape(&app)
        .await
        .contains("freight_write_admission_requests 0\n"));
}

#[tokio::test]
async fn busy_admission_registries_are_not_awaited_or_reported_as_zero() {
    let store = Store::new(None).await.expect("store");
    let reader = store.clone();
    let (observed, thread) = store.read_turns.with_registry_locked(|| {
        store.write_turns.with_registry_locked(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                let mut text = String::new();
                reader.admission_diagnostics().append(&mut text);
                let _ = tx.send(text);
            });
            (rx.recv_timeout(Duration::from_secs(1)), thread)
        })
    });
    thread.join().expect("diagnostic collector stopped");
    let text =
        observed.expect("diagnostics must complete while both actual registry locks remain held");
    assert!(text.contains("freight_admission_snapshot_available{lane=\"snapshot\"} 0\n"));
    assert!(text.contains("freight_admission_snapshot_available{lane=\"write\"} 0\n"));
    assert!(!text.contains("freight_admission_pending{lane=\"snapshot\"}"));
    assert!(!text
        .lines()
        .any(|line| line.starts_with("freight_write_admission_requests ")));
    assert!(text.contains("freight_admission_pending{lane=\"catalog\"} 0\n"));
}
