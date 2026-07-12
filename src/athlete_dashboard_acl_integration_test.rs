//! Integracja HTTP `GET /api/athletes/me/dashboard` — ACL (401 bez tokenu, dostęp z JWT).
#![allow(clippy::await_holding_lock)]

use std::sync::{Mutex, MutexGuard};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use jsonwebtoken::{EncodingKey, Header, encode};
use tempfile::tempdir;
use tower::ServiceExt;

use crate::middleware::auth::Claims;
use crate::models::Role;
use crate::{DatabaseBackend, create_app};

static ATHLETE_DASHBOARD_ACL_LOCK: Mutex<()> = Mutex::new(());

fn test_guard() -> MutexGuard<'static, ()> {
    ATHLETE_DASHBOARD_ACL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn jwt_for_roles(secret: &[u8], sub: &str, roles: Vec<Role>) -> String {
    let exp = (Utc::now() + Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: sub.to_string(),
        roles,
        exp,
        token_version: 0,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .expect("jwt encode")
}

async fn seeded_app() -> (axum::Router, String) {
    let _env_guard = crate::integration_test_env::integration_env_guard();
    unsafe {
        std::env::set_var("REBUILD_DB", "true");
    }
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("athlete_dashboard_acl.db");
    let jwt_secret = "integration-test-jwt-secret-key-min-len!!";

    let app = create_app(
        DatabaseBackend::Local(db_path),
        jwt_secret.to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    )
    .await
    .expect("create_app");

    unsafe {
        std::env::remove_var("REBUILD_DB");
    }

    (app, jwt_secret.to_string())
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn get_me_dashboard_without_token_returns_401() {
    let _guard = test_guard();
    let (app, _) = seeded_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/athletes/me/dashboard")
        .body(Body::empty())
        .expect("request build");

    let response = app.oneshot(req).await.expect("oneshot");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_me_dashboard_with_invalid_token_returns_401() {
    let _guard = test_guard();
    let (app, _) = seeded_app().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/athletes/me/dashboard")
        .header("authorization", "Bearer not-a-valid-jwt")
        .body(Body::empty())
        .expect("request build");

    let response = app.oneshot(req).await.expect("oneshot");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_me_dashboard_with_athlete_token_returns_200() {
    let _guard = test_guard();
    let (app, jwt_secret) = seeded_app().await;

    let trainer_token = crate::integration_test_http::login_token(
        &app,
        "Slavia",
        "SLAVIA2026",
    )
    .await;
    let req_athletes = Request::builder()
        .method("GET")
        .uri("/api/athletes/admin")
        .header("authorization", format!("Bearer {trainer_token}"))
        .body(Body::empty())
        .expect("request build");
    let resp_athletes = app.clone().oneshot(req_athletes).await.expect("oneshot");
    assert_eq!(resp_athletes.status(), StatusCode::OK);
    let athletes_json = response_json(resp_athletes).await;
    let athletes = athletes_json.as_array().expect("athletes array");
    assert!(!athletes.is_empty(), "seed athletes");

    let login = crate::integration_test_http::login(&app, "JakubGawron", "Jakubzofia2030?").await;
    let athlete_token = jwt_for_roles(
        jwt_secret.as_bytes(),
        &login.user_id,
        vec![Role::Athlete],
    );
    let req_dashboard = Request::builder()
        .method("GET")
        .uri("/api/athletes/me/dashboard")
        .header("authorization", format!("Bearer {athlete_token}"))
        .body(Body::empty())
        .expect("request build");

    let response = app.oneshot(req_dashboard).await.expect("oneshot");
    assert_eq!(response.status(), StatusCode::OK);

    let body = response_json(response).await;
    assert!(body.get("athlete").is_some());
    assert!(body.get("pending_results_count").is_some());
    assert!(body.get("calendar_entries").is_some());
}
