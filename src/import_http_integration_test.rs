//! Integracja HTTP `POST /api/import/data` (świeża baza + `REBUILD_DB=true`). Moduł w lib — bez osobnego binarka testowego.

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

static IMPORT_HTTP_LOCK: Mutex<()> = Mutex::new(());

fn test_guard() -> MutexGuard<'static, ()> {
    IMPORT_HTTP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn jwt_super(secret: &[u8]) -> String {
    let exp = (Utc::now() + Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: "integration-test-sub".into(),
        roles: vec![Role::SuperAdmin],
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .expect("jwt encode")
}

fn jwt_trainer(secret: &[u8]) -> String {
    let exp = (Utc::now() + Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: "integration-test-trainer".into(),
        roles: vec![Role::Trainer],
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .expect("jwt encode")
}

fn jwt_athlete(secret: &[u8], user_id: &str) -> String {
    let exp = (Utc::now() + Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: user_id.to_string(),
        roles: vec![Role::Athlete],
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .expect("jwt encode")
}

fn jwt_trainer_for_sub(secret: &[u8], sub: &str) -> String {
    let exp = (Utc::now() + Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: sub.to_string(),
        roles: vec![Role::Trainer],
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .expect("jwt encode")
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
async fn post_import_data_returns_three_sources_json() {
    let _guard = test_guard();

    // SAFETY: test jednowątkowy pod mutexem — ustawiamy REBUILD_DB tylko tu.
    unsafe {
        std::env::set_var("REBUILD_DB", "true");
    }
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("import_integration.db");
    let jwt_secret = "integration-test-jwt-secret-key-min-len!!";

    let app = create_app(
        DatabaseBackend::Local(db_path),
        jwt_secret.to_string(),
        String::new(),
        String::new(),
        String::new(),
    )
    .await
    .expect("create_app");

    unsafe {
        std::env::remove_var("REBUILD_DB");
    }

    let token = jwt_super(jwt_secret.as_bytes());
    let payload = serde_json::json!({ "dev_mode": false }).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/api/import/data")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(payload))
        .expect("request build");

    let response = app.oneshot(req).await.expect("oneshot");
    assert_eq!(response.status(), StatusCode::GONE);
}

#[tokio::test]
async fn trainer_can_read_system_metrics_and_event_feed() {
    let _guard = test_guard();
    // SAFETY: test jednowątkowy pod mutexem — ustawiamy REBUILD_DB tylko tu.
    unsafe {
        std::env::set_var("REBUILD_DB", "true");
    }
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("system_metrics_integration.db");
    let jwt_secret = "integration-test-jwt-secret-key-min-len!!";

    let app = create_app(
        DatabaseBackend::Local(db_path),
        jwt_secret.to_string(),
        String::new(),
        String::new(),
        String::new(),
    )
    .await
    .expect("create_app");

    unsafe {
        std::env::remove_var("REBUILD_DB");
    }

    let token = jwt_trainer(jwt_secret.as_bytes());
    let req_metrics = Request::builder()
        .method("GET")
        .uri("/api/system/metrics")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request build");

    let response_metrics = app.clone().oneshot(req_metrics).await.expect("oneshot");
    assert_eq!(response_metrics.status(), StatusCode::OK);

    let req_feed = Request::builder()
        .method("GET")
        .uri("/api/system/event-feed")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request build");

    let response_feed = app.oneshot(req_feed).await.expect("oneshot");
    assert_eq!(response_feed.status(), StatusCode::OK);
}

#[tokio::test]
async fn trainer_plan_and_athlete_recovery_flow_works() {
    let _guard = test_guard();
    // SAFETY: test jednowątkowy pod mutexem — ustawiamy REBUILD_DB tylko tu.
    unsafe {
        std::env::set_var("REBUILD_DB", "true");
    }
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("plan_recovery_flow.db");
    let jwt_secret = "integration-test-jwt-secret-key-min-len!!";

    let app = create_app(
        DatabaseBackend::Local(db_path),
        jwt_secret.to_string(),
        String::new(),
        String::new(),
        String::new(),
    )
    .await
    .expect("create_app");

    unsafe {
        std::env::remove_var("REBUILD_DB");
    }

    let trainer_token = jwt_trainer(jwt_secret.as_bytes());

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
    let chosen = athletes
        .iter()
        .find(|x| x.get("user_id").and_then(|v| v.as_str()).is_some())
        .expect("seed athlete with user_id");
    let athlete_id = chosen
        .get("id")
        .and_then(|v| v.as_str())
        .expect("athlete id")
        .to_string();
    let athlete_user_id = chosen
        .get("user_id")
        .and_then(|v| v.as_str())
        .expect("athlete user_id")
        .to_string();
    let trainer_existing_user_token = jwt_trainer_for_sub(jwt_secret.as_bytes(), &athlete_user_id);
    let athlete_token = jwt_athlete(jwt_secret.as_bytes(), &athlete_user_id);

    let req_create_plan = Request::builder()
        .method("POST")
        .uri("/api/training-plans")
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("Bearer {trainer_existing_user_token}"),
        )
        .body(Body::from(
            serde_json::json!({
                "athlete_id": athlete_id,
                "title": "Tydzień testowy",
                "week_start": "2026-05-05",
                "status": "active"
            })
            .to_string(),
        ))
        .expect("request build");
    let resp_create_plan = app.clone().oneshot(req_create_plan).await.expect("oneshot");
    assert_eq!(resp_create_plan.status(), StatusCode::OK);
    let plan_json = response_json(resp_create_plan).await;
    let plan_id = plan_json
        .get("id")
        .and_then(|v| v.as_str())
        .expect("plan id")
        .to_string();

    let req_athlete_progress = Request::builder()
        .method("PATCH")
        .uri(format!("/api/training-plans/{}/my-progress", plan_id))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {athlete_token}"))
        .body(Body::from(
            serde_json::json!({
                "status": "active",
                "progress_percent": 55,
                "athlete_note": "Trening wykonany zgodnie z planem"
            })
            .to_string(),
        ))
        .expect("request build");
    let resp_athlete_progress = app
        .clone()
        .oneshot(req_athlete_progress)
        .await
        .expect("oneshot");
    assert_eq!(resp_athlete_progress.status(), StatusCode::OK);

    let req_recovery = Request::builder()
        .method("POST")
        .uri("/api/recovery")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {athlete_token}"))
        .body(Body::from(
            serde_json::json!({
                "date": "2026-05-06",
                "sleep_hours": 8.0,
                "fatigue_level": 5,
                "soreness_level": 4,
                "readiness_level": 7,
                "note": "OK"
            })
            .to_string(),
        ))
        .expect("request build");
    let resp_recovery = app.clone().oneshot(req_recovery).await.expect("oneshot");
    assert_eq!(resp_recovery.status(), StatusCode::OK);

    let req_recovery_trainer = Request::builder()
        .method("GET")
        .uri(format!("/api/recovery/athlete/{}", athlete_id))
        .header("authorization", format!("Bearer {trainer_token}"))
        .body(Body::empty())
        .expect("request build");
    let resp_recovery_trainer = app.oneshot(req_recovery_trainer).await.expect("oneshot");
    assert_eq!(resp_recovery_trainer.status(), StatusCode::OK);
}
