//! Process-owned services and bounded, cooperative shutdown.
use crate::{
    api::{self, App},
    integrations::WorkerConfig,
};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::{future::Future, io, time::Duration};
use tokio::{net::TcpListener, sync::watch, task::JoinSet};

#[derive(Clone)]
pub struct Shutdown(watch::Sender<bool>);
impl Default for Shutdown {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}
impl Shutdown {
    pub fn request(&self) {
        // Retain the request even if no waiter has subscribed yet.
        self.0.send_replace(true);
    }
    pub fn requested(&self) -> bool {
        *self.0.borrow()
    }
    pub async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|stop| *stop).await;
    }
}

#[derive(Clone, Copy)]
pub struct ShutdownConfig {
    timeout: Duration,
}
impl ShutdownConfig {
    fn parse(raw: Option<&str>) -> Result<Self, String> {
        let raw = raw.unwrap_or("30000");
        if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(ms @ 1..=300_000) = raw.parse::<u64>() {
                return Ok(Self {
                    timeout: Duration::from_millis(ms),
                });
            }
        }
        Err("FREIGHT_SHUTDOWN_TIMEOUT_MS must be an integer from 1 to 300000".into())
    }
    pub fn from_env() -> Result<Self, String> {
        match std::env::var("FREIGHT_SHUTDOWN_TIMEOUT_MS") {
            Ok(raw) => Self::parse(Some(&raw)),
            Err(std::env::VarError::NotPresent) => Self::parse(None),
            Err(_) => Err("Invalid FREIGHT_SHUTDOWN_TIMEOUT_MS".into()),
        }
    }
}

// Register before opening storage or starting any service. Unix containers send
// SIGTERM; Ctrl-C sends SIGINT. Neither may leave a detached worker behind.
pub struct Signals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}
impl Signals {
    pub fn register() -> io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            interrupt: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?,
            #[cfg(unix)]
            terminate: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
        })
    }
    pub async fn recv(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let signal = tokio::select! {
                signal = self.interrupt.recv() => signal,
                signal = self.terminate.recv() => signal,
            };
            signal.ok_or_else(|| io::Error::other("Shutdown signal stream closed"))
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await
    }
}

pub(crate) async fn admission(
    State(shutdown): State<Shutdown>,
    request: Request,
    next: Next,
) -> Response {
    // Keep process liveness and authenticated diagnostics available on existing
    // connections during drain. Readiness and business traffic must fail closed.
    let diagnostic = matches!(request.method().as_str(), "GET" | "HEAD")
        && matches!(request.uri().path(), "/live" | "/metrics");
    if shutdown.requested() && !diagnostic {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [("retry-after", "1"), ("cache-control", "no-store")],
            Json(json!({"error":{"code":"service_draining","message":"Freight is restarting; retry with the same request key"}})),
        ).into_response();
    }
    next.run(request).await
}

type ServiceExit = (&'static str, Result<(), String>);

fn unexpected(result: Option<Result<ServiceExit, tokio::task::JoinError>>) -> String {
    match result {
        Some(Ok((name, Ok(())))) => format!("{name} service stopped unexpectedly"),
        Some(Ok((name, Err(error)))) => format!("{name} service failed: {error}"),
        Some(Err(error)) => format!("Service task failed: {error}"),
        None => "All service tasks stopped unexpectedly".into(),
    }
}

pub async fn serve(
    listener: TcpListener,
    app: App,
    worker_config: WorkerConfig,
    config: ShutdownConfig,
    shutdown: Shutdown,
    signal: impl Future<Output = io::Result<()>>,
) -> Result<(), String> {
    let observation = crate::observability::Observability::from_env(shutdown.clone())?;
    observation.validate_api_keys(app.keys.keys())?;
    let mut tasks = JoinSet::new();
    let integration_shutdown = shutdown.clone();
    let store = app.store.clone();
    let integrations = app.integrations.as_ref().clone();
    tasks.spawn(async move {
        crate::integration_worker::worker_until(
            store,
            integrations,
            worker_config,
            integration_shutdown,
        )
        .await;
        ("Integration", Ok(()))
    });
    let operations_shutdown = shutdown.clone();
    let store = app.store.clone();
    tasks.spawn(async move {
        crate::operations::worker_until(store, operations_shutdown).await;
        ("Operations", Ok(()))
    });
    let router = api::router_with_observability(app, observation);
    let http_shutdown = shutdown.clone();
    tasks.spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move { http_shutdown.cancelled().await })
            .await
            .map_err(|error| error.to_string());
        ("HTTP", result)
    });
    let mut failure = tokio::select! {
        biased;
        result = signal => result.err().map(|error| format!("Shutdown signal failed: {error}")),
        _ = shutdown.cancelled() => None,
        result = tasks.join_next() => Some(unexpected(result)),
    };
    shutdown.request();
    tracing::info!(
        timeout_ms = config.timeout.as_millis(),
        "Freight drain started"
    );
    let drained = tokio::time::timeout(config.timeout, async {
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((_, Ok(()))) => {}
                other => {
                    failure.get_or_insert_with(|| unexpected(Some(other)));
                }
            }
        }
    })
    .await;
    if drained.is_err() {
        // Dropping the owned JoinSet aborts async work; never clear a durable
        // lease or invent a failed partner outcome during forced shutdown.
        tasks.abort_all();
        return Err("Freight drain deadline exceeded; unfinished outcomes remain uncertain and durable leases are retained".into());
    }
    if let Some(error) = failure {
        return Err(error);
    }
    tracing::info!("Freight drain completed");
    Ok(())
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
