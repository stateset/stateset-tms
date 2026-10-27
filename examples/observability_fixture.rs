//! Emit an actual router scrape for independent Prometheus text parsing.
//! Runs in-process with synthetic credentials and no network or database.
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use stateset_freight::{
    api::{router_with_observability, App},
    auth::Credential,
    integrations::Integrations,
    lifecycle::Shutdown,
    observability::Observability,
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
        .init();
    let token = "fixture-monitor-synthetic-credential-123456789";
    let admin = "fixture-admin-synthetic-credential-123456789";
    let shutdown = Shutdown::default();
    let app = router_with_observability(
        App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store: Store::new(None).await?,
            keys: Arc::new(BTreeMap::from([(
                admin.into(),
                Credential::from("fixture"),
            )])),
            demo: true,
            integrations: Arc::new(Integrations::default()),
        },
        Observability::new(Some(token), shutdown.clone())?,
    );
    for (credential, expected) in [(admin, 200), ("wrong", 401)] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/access?secret=must-not-appear")
                    .header("authorization", format!("Bearer {credential}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status().as_u16(), expected);
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/documents/never-log-this-id/content?secret=must-not-appear")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status().as_u16(), 401);
    shutdown.request();
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/v1/access").body(Body::empty())?)
        .await?;
    assert_eq!(response.status().as_u16(), 503);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status().as_u16(), 200);
    print!(
        "{}",
        String::from_utf8(response.into_body().collect().await?.to_bytes().to_vec())?
    );
    Ok(())
}
