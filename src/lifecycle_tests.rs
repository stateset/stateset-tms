use super::*;
use axum::{body::Body, http::Request, middleware, routing::get, Router};
use http_body_util::BodyExt;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Semaphore;
use tower::ServiceExt;

#[test]
fn shutdown_configuration_requires_finite_positive_milliseconds() {
    assert_eq!(
        ShutdownConfig::parse(None).expect("default").timeout,
        Duration::from_secs(30)
    );
    for raw in ["1", "30000", "300000"] {
        assert_eq!(
            ShutdownConfig::parse(Some(raw))
                .expect("valid")
                .timeout
                .as_millis()
                .to_string(),
            raw
        );
    }
    for raw in [
        "",
        "0",
        "300001",
        "-1",
        "+1",
        " 1",
        "1 ",
        "1.5",
        "999999999999999999999999999",
    ] {
        assert!(ShutdownConfig::parse(Some(raw)).is_err(), "{raw}");
    }
}

#[tokio::test]
async fn shutdown_wakes_all_waiters_and_retains_requests_for_late_subscribers() {
    let shutdown = Shutdown::default();
    let mut tasks = JoinSet::new();
    for _ in 0..20 {
        let shutdown = shutdown.clone();
        tasks.spawn(async move { shutdown.cancelled().await });
    }
    shutdown.request();
    shutdown.request();
    assert!(shutdown.requested());
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(result) = tasks.join_next().await {
            result.expect("waiter");
        }
        shutdown.cancelled().await;
    })
    .await
    .expect("all waiters and late subscription observe shutdown");
}

#[tokio::test]
async fn draining_rejects_new_requests_before_handlers_but_retains_admitted_work() {
    let shutdown = Shutdown::default();
    let started = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/health",
            get({
                let started = started.clone();
                let release = release.clone();
                let calls = calls.clone();
                move || {
                    let started = started.clone();
                    let release = release.clone();
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        started.add_permits(1);
                        release.acquire().await.expect("release").forget();
                        "original response"
                    }
                }
            }),
        )
        .layer(middleware::from_fn_with_state(shutdown.clone(), admission));
    let original = tokio::spawn(
        router.clone().oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        ),
    );
    started.acquire().await.expect("handler entered").forget();
    shutdown.request();
    for path in ["/health", "/webhooks/edi/demo", "/v1/loads"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["retry-after"], "1");
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).expect("error")["error"]["code"],
            "service_draining"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        !original.is_finished(),
        "drain must not cancel an admitted request"
    );
    release.add_permits(1);
    let response = original.await.expect("original task").expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .expect("original body")
            .to_bytes(),
        "original response"
    );
}
