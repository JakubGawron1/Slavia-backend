//! Wspólne helpery HTTP dla testów integracyjnych (login ze seeda REBUILD_DB).
#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::models::Role;

#[derive(serde::Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user_id: String,
    pub roles: Vec<Role>,
}

pub async fn login_token(app: &axum::Router, username: &str, password: &str) -> String {
    login(app, username, password).await.token
}

pub async fn login(app: &axum::Router, username: &str, password: &str) -> LoginResponse {
    let payload = serde_json::json!({ "username": username, "password": password }).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(payload))
        .expect("request build");

    let response = app.clone().oneshot(req).await.expect("oneshot");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "login failed for {username}"
    );

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("login json")
}
