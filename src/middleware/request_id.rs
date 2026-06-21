//! Identyfikator żądania (`x-request-id`) — korelacja logów TraceLayer z odpowiedziami błędów.

use axum::{
    body::Body,
    http::{Request, Response, header},
    middleware::Next,
};
use http_body_util::BodyExt;
use uuid::Uuid;

use crate::api_error::ErrorBody;

/// UUID v4 przypisany do jednego żądania HTTP (extension + nagłówek odpowiedzi).
#[derive(Clone, Debug)]
pub struct RequestId(Uuid);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn request_id_from_extensions(
    extensions: &axum::http::Extensions,
) -> Option<&RequestId> {
    extensions.get::<RequestId>()
}

/// Ustawia `RequestId` w extensions, nagłówek `x-request-id` oraz opcjonalnie pole JSON przy 5xx.
pub async fn request_id_middleware(
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let request_id = RequestId::new();
    request.extensions_mut().insert(request_id.clone());

    let mut response = next.run(request).await;

    if let Ok(value) = header::HeaderValue::from_str(&request_id.as_str()) {
        response
            .headers_mut()
            .insert(header::HeaderName::from_static("x-request-id"), value);
    }

    if response.status().is_server_error() {
        response = inject_request_id_into_error_json(response, &request_id).await;
    }

    response
}

async fn inject_request_id_into_error_json(
    response: Response<Body>,
    request_id: &RequestId,
) -> Response<Body> {
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    if !is_json {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let Ok(collected) = body.collect().await else {
        return Response::from_parts(parts, Body::empty());
    };
    let bytes = collected.to_bytes();

    let Ok(mut body_value) = serde_json::from_slice::<ErrorBody>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };

    if body_value.request_id.is_none() {
        body_value.request_id = Some(request_id.as_str());
    }

    let Ok(encoded) = serde_json::to_vec(&body_value) else {
        return Response::from_parts(parts, Body::from(bytes));
    };

    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(encoded))
}
