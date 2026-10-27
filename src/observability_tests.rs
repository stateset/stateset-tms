use super::*;
use crate::{
    api::{self, App},
    auth::Credential,
    integrations::Integrations,
    store::Store,
};
use axum::{middleware, routing::get, Router};
use http_body_util::BodyExt;
use tokio::sync::Semaphore;
use tower::ServiceExt;

const TOKEN: &str = "monitor-only-synthetic-credential-123456789";
const ADMIN: &str = "tenant-admin-synthetic-credential-123456789";

async fn router(state: Observability) -> Router {
    api::router_with_observability(
        App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store: Store::new(None).await.expect("memory store"),
            keys: Arc::new(BTreeMap::from([(
                ADMIN.into(),
                Credential::from("private-tenant"),
            )])),
            demo: true,
            integrations: Arc::new(Integrations::default()),
        },
        state,
    )
}

async fn call(router: &Router, path: &str, method: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().uri(path).method(method);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

async fn body(response: Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec(),
    )
    .expect("UTF-8")
}

#[test]
fn metrics_configuration_rejects_invalid_credentials_without_disclosing_them() {
    for token in [
        "short-private-token".to_owned(),
        "x".repeat(513),
        format!("{}\n", "x".repeat(32)),
        "é".repeat(32),
    ] {
        let result = Observability::new(Some(&token), Shutdown::default());
        assert_eq!(result.err().expect("invalid credential"), TOKEN_ERROR);
    }
    for token in [None, Some("")] {
        assert!(Observability::new(token, Shutdown::default())
            .expect("disabled")
            .inner
            .token_digest
            .is_none());
    }
    let observation = Observability::new(Some(TOKEN), Shutdown::default()).expect("monitor");
    assert!(observation
        .validate_api_keys([ADMIN.to_owned()].iter())
        .is_ok());
    let error = observation
        .validate_api_keys([TOKEN.to_owned()].iter())
        .expect_err("a business token cannot authorize process metrics");
    assert!(!error.contains(TOKEN));
}

#[tokio::test]
async fn probes_and_metrics_follow_drain_without_admitting_business_requests() {
    let shutdown = Shutdown::default();
    let observation = Observability::new(Some(TOKEN), shutdown.clone()).expect("monitor");
    let router = router(observation.clone()).await;
    for path in ["/live", "/ready", "/health"] {
        let response = call(&router, path, "GET", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    shutdown.request();
    for path in [
        "/ready",
        "/health",
        "/v1/access",
        "/webhooks/edi/private-tenant",
    ] {
        let response = call(&router, path, "GET", Some(ADMIN)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["retry-after"], "1");
    }
    assert_eq!(
        call(&router, "/live", "GET", None).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "/live", "HEAD", None).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "/live", "POST", None).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        call(&router, "/metrics", "GET", Some(ADMIN)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = call(&router, "/metrics", "GET", Some(TOKEN)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let text = body(response).await;
    assert!(text.contains("freight_process_draining 1\n"));
    assert!(text.contains(
        "freight_http_requests_total{route=\"/v1/access\",method=\"GET\",outcome=\"5xx\"} 1\n"
    ));
    assert!(!text.contains("route=\"/live\""));
    assert!(!text.contains("route=\"/ready\""));
    assert!(!text.contains("route=\"/health\""));
    assert!(!text.contains("route=\"/metrics\""));
    assert!(!text.contains("private-tenant"));
    assert_eq!(observation.inner.in_flight.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn metrics_require_the_dedicated_credential_and_disable_by_default() {
    let disabled = router(Observability::default()).await;
    assert_eq!(
        call(&disabled, "/metrics", "GET", Some(TOKEN))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let observation = Observability::new(Some(TOKEN), Shutdown::default()).expect("monitor");
    let enabled = router(observation).await;
    for token in [
        None,
        Some(ADMIN),
        Some("incorrect-monitor-credential-123456789"),
    ] {
        let response = call(&enabled, "/metrics", "GET", token).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
        assert!(body(response).await.is_empty());
    }
    assert_eq!(
        call(&enabled, &format!("/metrics?token={TOKEN}"), "GET", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let duplicate = Request::builder()
        .uri("/metrics")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .expect("duplicate request");
    assert_eq!(
        enabled
            .clone()
            .oneshot(duplicate)
            .await
            .expect("response")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    // A process monitoring credential has no business permissions.
    assert_eq!(
        call(&enabled, "/v1/access", "GET", Some(TOKEN))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = call(&enabled, "/metrics", "GET", Some(TOKEN)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/plain; version=0.0.4; charset=utf-8"
    );
    assert!(!body(response).await.contains(TOKEN));
}

#[tokio::test]
async fn metric_labels_collapse_untrusted_paths_queries_and_methods() {
    let observation = Observability::new(Some(TOKEN), Shutdown::default()).expect("monitor");
    let router = router(observation.clone()).await;
    for i in 0..100 {
        let path = format!("/v1/documents/private-document-{i}/content?secret=private-query-{i}");
        assert_eq!(
            call(&router, &path, "GET", None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let path = format!("/unknown-private-{i}?credential=private-query-{i}");
        assert_eq!(
            call(&router, &path, &format!("CUSTOM{i}"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(&router, "/v1/access", "GET", Some(ADMIN))
            .await
            .status(),
        StatusCode::OK
    );
    let text = body(call(&router, "/metrics", "GET", Some(TOKEN)).await).await;
    for secret in ["private-", "CUSTOM", TOKEN, ADMIN] {
        assert!(
            !text.contains(secret),
            "metric labels must not contain {secret}"
        );
    }
    assert!(text.contains("freight_http_requests_total{route=\"/v1/documents/{id}/content\",method=\"GET\",outcome=\"4xx\"} 100\n"));
    assert!(text.contains(
        "freight_http_requests_total{route=\"unmatched\",method=\"OTHER\",outcome=\"4xx\"} 100\n"
    ));
    assert_eq!(observation.inner.series.lock().expect("series").len(), 3);
}

#[tokio::test]
async fn cancelled_request_releases_gauge_and_records_exactly_one_outcome() {
    let observation = Observability::new(Some(TOKEN), Shutdown::default()).expect("monitor");
    let started = Arc::new(Semaphore::new(0));
    let router = Router::new()
        .route(
            "/wait/{id}",
            get({
                let started = started.clone();
                move || {
                    let started = started.clone();
                    async move {
                        started.add_permits(1);
                        std::future::pending::<()>().await;
                        StatusCode::OK
                    }
                }
            }),
        )
        .layer(middleware::from_fn_with_state(observation.clone(), observe));
    let task = tokio::spawn(
        router.oneshot(
            Request::builder()
                .uri("/wait/private-id")
                .body(Body::empty())
                .expect("request"),
        ),
    );
    started.acquire().await.expect("started").forget();
    assert_eq!(observation.inner.in_flight.load(Ordering::Relaxed), 1);
    task.abort();
    assert!(task.await.expect_err("cancelled task").is_cancelled());
    assert_eq!(observation.inner.in_flight.load(Ordering::Relaxed), 0);
    let text = observation.render();
    assert!(text.contains("freight_http_requests_total{route=\"/wait/{id}\",method=\"GET\",outcome=\"cancelled\"} 1\n"));
    assert!(!text.contains("outcome=\"2xx\""));
    assert!(!text.contains("private-id"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_have_exact_counts_and_cumulative_histograms() {
    let observation = Observability::new(Some(TOKEN), Shutdown::default()).expect("monitor");
    let router = router(observation.clone()).await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let router = router.clone();
        tasks.spawn(async move {
            call(&router, "/v1/access", "GET", Some(ADMIN))
                .await
                .status()
        });
    }
    while let Some(result) = tasks.join_next().await {
        assert_eq!(result.expect("task"), StatusCode::OK);
    }
    let text = body(call(&router, "/metrics", "GET", Some(TOKEN)).await).await;
    let prefix = "route=\"/v1/access\",method=\"GET\",outcome=\"2xx\"";
    assert!(text.contains(&format!("freight_http_requests_total{{{prefix}}} 32\n")));
    assert!(text.contains(&format!(
        "freight_http_response_headers_duration_seconds_count{{{prefix}}} 32\n"
    )));
    assert!(text.contains(&format!(
        "freight_http_response_headers_duration_seconds_bucket{{{prefix},le=\"+Inf\"}} 32\n"
    )));
    let buckets: Vec<u64> = text
        .lines()
        .filter(|line| line.starts_with("freight_http_response_headers_duration_seconds_bucket{"))
        .map(|line| {
            line.rsplit(' ')
                .next()
                .expect("value")
                .parse()
                .expect("count")
        })
        .collect();
    assert_eq!(buckets.len(), BUCKETS.len() + 1);
    assert!(buckets.windows(2).all(|pair| pair[0] <= pair[1]));
    assert_eq!(observation.inner.in_flight.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn readiness_checks_storage_with_a_deadline_and_observes_shutdown_during_check() {
    let shutdown = Shutdown::default();
    assert!(ready(&shutdown, async { true }).await);
    assert!(!ready(&shutdown, async { false }).await);
    assert!(!tokio::time::timeout(
        Duration::from_secs(3),
        ready(&shutdown, std::future::pending())
    )
    .await
    .expect("bounded readiness"));
    let started = Arc::new(Semaphore::new(0));
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        let started = started.clone();
        async move {
            ready(&shutdown, async {
                started.add_permits(1);
                std::future::pending().await
            })
            .await
        }
    });
    started.acquire().await.expect("checking storage").forget();
    shutdown.request();
    assert!(!tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .expect("shutdown interrupts check")
        .expect("task"));
    assert!(
        !ready(&shutdown, async {
            panic!("draining must not query storage")
        })
        .await
    );
}
