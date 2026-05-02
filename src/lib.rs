//! Współdzielona logika HTTP — używana przez `main` (Axum/Tokio) i testy.

use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

pub mod db;
pub mod dto;
pub mod middleware;
pub mod models;
pub mod notifications;
pub mod repos;
pub mod router;
pub mod routes;
pub mod state;

pub(crate) mod api_error;
mod sql_row;
pub mod cloudinary;
mod external_calendar_sync;

use state::AppState;

/// Buduje router Axum (Turso/libsql + JWT). Bez `Box::pin` — mniejsze ryzyko problemów ze stosem na Windows.
pub async fn create_app(
    db_url: &str,
    db_token: &str,
    jwt_secret: String,
    cloudinary_cloud_name: String,
    cloudinary_api_key: String,
    cloudinary_api_secret: String,
) -> Result<axum::Router, Box<dyn std::error::Error + Send + Sync>> {
    let client = libsql::Builder::new_remote(db_url.to_string(), db_token.to_string())
        .build()
        .await?;

    let conn = client.connect()?;

    db::init_db(&conn).await?;

    let state = AppState {
        db: Arc::new(conn),
        jwt_secret,
        cloudinary_cloud_name,
        cloudinary_api_key,
        cloudinary_api_secret,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Ok(router::build_router(state, cors))
}
