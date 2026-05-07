//! Współdzielona logika HTTP — używana przez `main` (Axum/Tokio) i testy.

use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

pub mod db;
pub mod dto;
pub mod middleware;
pub mod models;
pub mod notifications;
pub mod audit;
pub mod repos;
pub mod router;
pub mod routes;
pub mod state;

pub(crate) mod api_error;
mod sql_row;
pub mod cloudinary;
mod external_calendar_sync;

#[cfg(test)]
mod import_http_integration_test;

use state::AppState;

/// Skąd brać bazę: lokalny plik SQLite (dev) albo Turso przez HTTP (`new_remote`).
#[derive(Debug, Clone)]
pub enum DatabaseBackend {
    Local(PathBuf),
    Remote {
        url: String,
        auth_token: String,
    },
}

async fn connect_database(
    backend: DatabaseBackend,
) -> Result<libsql::Connection, Box<dyn std::error::Error + Send + Sync>> {
    match backend {
        DatabaseBackend::Local(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let db = libsql::Builder::new_local(path).build().await?;
            Ok(db.connect()?)
        }
        DatabaseBackend::Remote { url, auth_token } => {
            let db = libsql::Builder::new_remote(url, auth_token).build().await?;
            Ok(db.connect()?)
        }
    }
}

/// Buduje router Axum (libsql: SQLite lokalnie lub Turso zdalnie + JWT).
pub async fn create_app(
    database: DatabaseBackend,
    jwt_secret: String,
    cloudinary_cloud_name: String,
    cloudinary_api_key: String,
    cloudinary_api_secret: String,
) -> Result<axum::Router, Box<dyn std::error::Error + Send + Sync>> {
    let conn = connect_database(database).await?;

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
