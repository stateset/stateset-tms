//! Process diagnostics. No tenant identifiers, request values or raw URLs are labels.
use crate::lifecycle::Shutdown;
use axum::{
    body::Body,
    extract::{MatchedPath, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt::Write,
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const BUCKETS: [f64; 15] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
];
const TOKEN_ERROR: &str = "FREIGHT_METRICS_TOKEN must contain 32–512 visible ASCII characters";

#[derive(Clone)]
pub struct Observability {
    pub(crate) shutdown: Shutdown,
    inner: Arc<Inner>,
}

struct Inner {
    token_digest: Option<[u8; 32]>,
    started: Instant,
    in_flight: AtomicU64,
    series: Mutex<BTreeMap<Labels, Series>>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Labels {
    route: String,
    method: &'static str,
    outcome: &'static str,
}

#[derive(Clone, Default)]
struct Series {
    count: u64,
    seconds: f64,
    buckets: [u64; BUCKETS.len()],
}

impl Default for Observability {
    fn default() -> Self {
        Self::new(None, Shutdown::default()).expect("disabled metrics configuration is valid")
    }
}

impl Observability {
    /// A dedicated process-wide credential; tenant administrator keys do not
    /// authorize metrics for other tenants. Empty configuration disables export.
    pub fn new(token: Option<&str>, shutdown: Shutdown) -> Result<Self, String> {
        let token_digest = match token.filter(|token| !token.is_empty()) {
            Some(token)
                if (32..=512).contains(&token.len())
                    && token.bytes().all(|byte| byte.is_ascii_graphic()) =>
            {
                Some(Sha256::digest(token.as_bytes()).into())
            }
            Some(_) => return Err(TOKEN_ERROR.into()),
            None => None,
        };
        Ok(Self {
            shutdown,
            inner: Arc::new(Inner {
                token_digest,
                started: Instant::now(),
                in_flight: AtomicU64::new(0),
                series: Mutex::new(BTreeMap::new()),
            }),
        })
    }

    pub fn from_env(shutdown: Shutdown) -> Result<Self, String> {
        match std::env::var("FREIGHT_METRICS_TOKEN") {
            Ok(value) => Self::new(Some(&value), shutdown),
            Err(std::env::VarError::NotPresent) => Self::new(None, shutdown),
            Err(_) => Err(TOKEN_ERROR.into()),
        }
    }

    pub(crate) fn validate_api_keys<'a>(
        &self,
        keys: impl Iterator<Item = &'a String>,
    ) -> Result<(), String> {
        if let Some(expected) = self.inner.token_digest {
            for key in keys {
                let received: [u8; 32] = Sha256::digest(key.as_bytes()).into();
                if received == expected {
                    return Err(
                        "FREIGHT_METRICS_TOKEN must be distinct from every freight API credential"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = self.inner.token_digest else {
            return false;
        };
        if headers.get_all("authorization").iter().count() != 1 {
            return false;
        }
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|token| (32..=512).contains(&token.len()))
            .is_some_and(|token| {
                let received = Sha256::digest(token.as_bytes());
                received
                    .iter()
                    .zip(expected)
                    .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                    == 0
            })
    }

    fn render(&self) -> String {
        // Release the lock before formatting. Request completion never waits on
        // network I/O or on formatting a complete scrape.
        let series = self
            .inner
            .series
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut text = String::new();
        let _ = writeln!(text, "# HELP freight_process_uptime_seconds Seconds since this process monitor started.\n# TYPE freight_process_uptime_seconds gauge\nfreight_process_uptime_seconds {}", self.inner.started.elapsed().as_secs_f64());
        let _ = writeln!(text, "# HELP freight_process_draining Whether this process has requested shutdown.\n# TYPE freight_process_draining gauge\nfreight_process_draining {}", u8::from(self.shutdown.requested()));
        let _ = writeln!(text, "# HELP freight_http_in_flight Requests awaiting response headers, excluding diagnostics.\n# TYPE freight_http_in_flight gauge\nfreight_http_in_flight {}", self.inner.in_flight.load(Ordering::Relaxed));
        text.push_str("# HELP freight_http_requests_total Requests ending in response headers or cancellation, excluding diagnostics.\n# TYPE freight_http_requests_total counter\n");
        for (labels, value) in &series {
            let _ = writeln!(
                text,
                "freight_http_requests_total{{{}}} {}",
                labels.render(),
                value.count
            );
        }
        text.push_str("# HELP freight_http_response_headers_duration_seconds Time until response headers or cancellation, excluding diagnostics; not body transfer time.\n# TYPE freight_http_response_headers_duration_seconds histogram\n");
        for (labels, value) in &series {
            let labels = labels.render();
            for (bound, count) in BUCKETS.iter().zip(value.buckets) {
                let _ = writeln!(text, "freight_http_response_headers_duration_seconds_bucket{{{labels},le=\"{bound}\"}} {count}");
            }
            let _ = writeln!(
                text,
                "freight_http_response_headers_duration_seconds_bucket{{{labels},le=\"+Inf\"}} {}",
                value.count
            );
            let _ = writeln!(
                text,
                "freight_http_response_headers_duration_seconds_sum{{{labels}}} {}",
                value.seconds
            );
            let _ = writeln!(
                text,
                "freight_http_response_headers_duration_seconds_count{{{labels}}} {}",
                value.count
            );
        }
        text
    }
}

impl Labels {
    fn render(&self) -> String {
        let route = self
            .route
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n");
        format!(
            "route=\"{route}\",method=\"{}\",outcome=\"{}\"",
            self.method, self.outcome
        )
    }
}

fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        Method::TRACE => "TRACE",
        Method::CONNECT => "CONNECT",
        _ => "OTHER",
    }
}

fn route_label(request: &Request) -> &str {
    request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
}

pub(crate) fn request_span(request: &axum::http::Request<Body>) -> tracing::Span {
    tracing::info_span!(
        "http_request",
        method = method_label(request.method()),
        route = route_label(request)
    )
}

struct Observation {
    state: Observability,
    labels: Labels,
    started: Instant,
}

impl Drop for Observation {
    fn drop(&mut self) {
        let seconds = self.started.elapsed().as_secs_f64();
        let mut series = self
            .state
            .inner
            .series
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let value = series.entry(self.labels.clone()).or_default();
        value.count = value.count.saturating_add(1);
        value.seconds += seconds;
        for (count, bound) in value.buckets.iter_mut().zip(BUCKETS) {
            if seconds <= bound {
                *count = count.saturating_add(1);
            }
        }
        self.state.inner.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) async fn observe(
    State(state): State<Observability>,
    request: Request,
    next: Next,
) -> Response {
    let route = route_label(&request);
    if state.inner.token_digest.is_none()
        || matches!(route, "/live" | "/ready" | "/health" | "/metrics")
    {
        return next.run(request).await;
    }
    let labels = Labels {
        route: route.to_owned(),
        method: method_label(request.method()),
        outcome: "cancelled",
    };
    state.inner.in_flight.fetch_add(1, Ordering::Relaxed);
    let mut observation = Observation {
        state,
        labels,
        started: Instant::now(),
    };
    let response = next.run(request).await;
    observation.labels.outcome = match response.status().as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    };
    response
}

pub(crate) async fn live() -> Response {
    (
        [("cache-control", "no-store")],
        Json(json!({"status":"ok","service":"stateset-freight"})),
    )
        .into_response()
}

pub(crate) async fn ready(shutdown: &Shutdown, storage: impl Future<Output = bool>) -> bool {
    if shutdown.requested() {
        return false;
    }
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => false,
        result = tokio::time::timeout(Duration::from_secs(1), storage) => {
            result.unwrap_or(false) && !shutdown.requested()
        }
    }
}

pub(crate) async fn metrics(
    Extension(state): Extension<Observability>,
    State(app): State<crate::api::App>,
    headers: HeaderMap,
) -> Response {
    let mut response = if state.inner.token_digest.is_none() {
        StatusCode::NOT_FOUND.into_response()
    } else if !state.authorized(&headers) {
        (StatusCode::UNAUTHORIZED, [("www-authenticate", "Bearer")]).into_response()
    } else {
        let mut text = state.render();
        app.store.admission_diagnostics().append(&mut text);
        (
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            text,
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("static header"));
    response
}

#[cfg(test)]
#[path = "observability_tests.rs"]
mod tests;
