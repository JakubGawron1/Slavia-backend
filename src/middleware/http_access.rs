//! Jednoliniowy access log HTTP — method, path, status, latency, request_id (RODO: bez query/body).

use std::time::Instant;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    middleware::Next,
    response::Response,
};

fn request_id_from_response(response: &Response<Body>) -> Option<String> {
    response
        .headers()
        .get(header::HeaderName::from_static("x-request-id"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn status_code(status: StatusCode) -> u16 {
    status.as_u16()
}

/// Loguje każde żądanie po zakończeniu (jeden wpis, bez duplikatów z tower-http).
pub async fn http_access_log_middleware(request: Request<Body>, next: Next) -> Response<Body> {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    let started = Instant::now();

    let response = next.run(request).await;

    let latency_ms = started.elapsed().as_millis();
    let status = response.status();
    let code = status_code(status);
    let request_id = request_id_from_response(&response).unwrap_or_else(|| "-".to_string());

    if status.is_server_error() {
        slavia_error!(
            "http",
            "HTTP request returned 5xx",
            "HF: retry after cold start; persist: check Turso sync and DB pool",
            http_method = %method,
            http_path = %path,
            status = code,
            latency_ms,
            request_id = %request_id,
        );
    } else if status.is_client_error() {
        slavia_warn!(
            "http",
            "HTTP request returned 4xx",
            "verify auth token, ACL and request payload",
            http_method = %method,
            http_path = %path,
            status = code,
            latency_ms,
            request_id = %request_id,
        );
    } else if latency_ms >= 2000 {
        slavia_warn!(
            "http",
            "HTTP request is slow",
            "profile DB queries or HF Space cold start",
            http_method = %method,
            http_path = %path,
            status = code,
            latency_ms,
            request_id = %request_id,
        );
    } else {
        slavia_debug!(
            "http",
            "HTTP request completed",
            "no action needed",
            http_method = %method,
            http_path = %path,
            status = code,
            latency_ms,
            request_id = %request_id,
        );
    }

    response
}
