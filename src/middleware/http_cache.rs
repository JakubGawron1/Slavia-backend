//! Nagłówki `Cache-Control` dla publicznych odczytów GET (CDN + przeglądarka).

use axum::{
    body::Body,
    http::{Method, Request, Response, header},
    middleware::Next,
};

fn cache_header(value: &str) -> Option<header::HeaderValue> {
    header::HeaderValue::from_str(value).ok()
}

/// Ustawia sensowne TTL tylko na bezpiecznych, publicznych GET-ach (bez JWT w odpowiedzi).
pub async fn cache_control_middleware(
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    let mut response = next.run(request).await;

    if method != Method::GET {
        return response;
    }

    let policy = if path == "/api/system/ping" || path == "/api/health" {
        Some("no-store")
    } else if path == "/api/system/openapi.json" {
        Some("public, max-age=86400, stale-while-revalidate=3600")
    } else if path == "/api/system/mobile-releases/latest" {
        Some("public, max-age=300, stale-while-revalidate=600")
    } else if path == "/api/challenges/monthly-training-sessions" {
        Some("public, max-age=120, stale-while-revalidate=300")
    } else if path == "/api/athletes" || path.starts_with("/api/athletes/") {
        // Tylko publiczne profile — bez /me, /admin, /training-log, itd.
        let private_segment = path.contains("/me")
            || path.contains("/admin")
            || path.contains("/training-log")
            || path.contains("/my-calendar")
            || path.contains("/link")
            || path.contains("/attach-user")
            || path.contains("/detach-user")
            || path.contains("/timeline")
            || path.contains("/competitions");
        if private_segment {
            None
        } else {
            Some("public, max-age=120, stale-while-revalidate=300")
        }
    } else if path.starts_with("/api/results/public-board") {
        Some("public, max-age=60, stale-while-revalidate=180")
    } else if path == "/api/announcements" || path == "/api/announcements/" {
        Some("public, max-age=120, stale-while-revalidate=300")
    } else if path == "/api/gallery" || path == "/api/gallery/" {
        Some("public, max-age=180, stale-while-revalidate=600")
    } else if path == "/api/posts" || path == "/api/posts/" {
        Some("public, max-age=120, stale-while-revalidate=300")
    } else if path.starts_with("/api/cms/variables")
        || path.starts_with("/api/cms/page/")
        || path.starts_with("/api/cms/navigation")
        || path.starts_with("/api/cms/pages")
    {
        Some("public, max-age=60, stale-while-revalidate=120")
    } else if path.starts_with("/api/cms/")
        || path.contains("/manage")
        || path.contains("/admin")
        || path.starts_with("/api/auth/")
    {
        Some("private, no-store")
    } else if path == "/api/competitions/" || path.starts_with("/api/competitions/") {
        Some("public, max-age=120, stale-while-revalidate=300")
    } else {
        None
    };

    if let Some(value) = policy
        && let Some(h) = cache_header(value) {
            response.headers_mut().insert(header::CACHE_CONTROL, h);
        }

    response
}
