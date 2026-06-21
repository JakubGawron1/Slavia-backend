//! Lekki endpoint Prometheus `GET /metrics` — liczniki żądań HTTP i błędów (stub OBS-1 / H-4).
//! Włączany opcjonalnie przez `PROMETHEUS_METRICS=1`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{header, Request},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Czy wystawić `GET /metrics` i middleware zliczające żądania.
pub fn prometheus_metrics_enabled() -> bool {
    std::env::var("PROMETHEUS_METRICS")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[derive(Default)]
pub struct HttpMetrics {
    requests: AtomicU64,
    errors: AtomicU64,
}

impl HttpMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn inc_requests(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_errors(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn render_prometheus(&self) -> String {
        let reqs = self.requests.load(Ordering::Relaxed);
        let errs = self.errors.load(Ordering::Relaxed);
        format!(
            "# HELP slavia_http_requests_total Total HTTP requests served.\n\
             # TYPE slavia_http_requests_total counter\n\
             slavia_http_requests_total {reqs}\n\
             # HELP slavia_http_errors_total HTTP responses with status >= 400.\n\
             # TYPE slavia_http_errors_total counter\n\
             slavia_http_errors_total {errs}\n"
        )
    }
}

pub async fn track_http_metrics(
    State(state): State<crate::state::AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    state.http_metrics.inc_requests();
    let response = next.run(request).await;
    if response.status().is_client_error() || response.status().is_server_error() {
        state.http_metrics.inc_errors();
    }
    response
}

pub async fn prometheus_handler(State(state): State<crate::state::AppState>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.http_metrics.render_prometheus(),
    )
}
