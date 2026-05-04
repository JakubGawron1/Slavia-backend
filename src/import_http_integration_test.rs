//! Integracja HTTP `POST /api/import/data` (świeża baza + `REBUILD_DB=true`). Moduł w lib — bez osobnego binarka testowego.

use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use jsonwebtoken::{encode, EncodingKey, Header};
use tempfile::tempdir;
use tower::ServiceExt;

use crate::middleware::auth::Claims;
use crate::models::Role;
use crate::{create_app, DatabaseBackend};

static IMPORT_HTTP_LOCK: Mutex<()> = Mutex::new(());

fn jwt_super(secret: &[u8]) -> String {
    let exp = (Utc::now() + Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: "integration-test-sub".into(),
        roles: vec![Role::SuperAdmin],
        exp,
    };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret)).expect("jwt encode")
}

#[tokio::test]
async fn post_import_data_returns_three_sources_json() {
    let _guard = IMPORT_HTTP_LOCK.lock().expect("lock");

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
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.expect("collect body").to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    let arr = v.as_array().expect("array response");
    assert_eq!(arr.len(), 3);
    for row in arr {
        assert!(row.get("source").is_some());
        assert!(row.get("rows_parsed").is_some());
    }
}
