//! Współdzielona logika HTTP — używana przez `main` (Axum/Tokio) i testy.

use std::path::PathBuf;
use tower_http::cors::{Any, CorsLayer};

pub mod audit;
pub mod chat_cleanup;
pub mod db;
pub mod dto;
pub mod middleware;
pub mod models;
pub mod notifications;
pub mod payments_scheduler;
pub mod worker_metrics;
pub mod repos;
pub mod router;
pub mod routes;
pub mod state;

pub(crate) mod api_error;
pub mod cloudinary;
pub mod cms_github;
mod external_calendar_sync;
mod login_throttle;
mod post_throttle;
mod sql_row;

#[cfg(test)]
mod import_http_integration_test;

use state::AppState;
use state::Db;

/// Skąd brać bazę: lokalny plik SQLite (dev) albo Turso przez HTTP (`new_remote`).
#[derive(Debug, Clone)]
pub enum DatabaseBackend {
    Local(PathBuf),
    Remote { url: String, auth_token: String },
}

/// Buduje router Axum (libsql: SQLite lokalnie lub Turso zdalnie + JWT).
pub async fn create_app(
    database: DatabaseBackend,
    jwt_secret: String,
    cloudinary_cloud_name: String,
    cloudinary_api_key: String,
    cloudinary_api_secret: String,
) -> Result<axum::Router, Box<dyn std::error::Error + Send + Sync>> {
    let db = Db::new(database).await?;
    let init_conn = db.raw().await;
    db::init_db(init_conn.as_ref()).await?;

    let worker_metrics: std::sync::Arc<worker_metrics::WorkerMetrics> =
        std::sync::Arc::new(worker_metrics::WorkerMetrics::new());

    // Uruchom „best-effort" jednorazowe czyszczenie wątków na starcie (nie blokuje startu).
    {
        let db_for_initial = db.clone();
        let wm = worker_metrics.clone();
        tokio::spawn(async move {
            let t0 = std::time::Instant::now();
            match chat_cleanup::prune_inactive_chat_threads(
                &db_for_initial,
                chat_cleanup::CHAT_INACTIVITY_DAYS,
            )
            .await
            {
                Ok(n) => {
                    wm.record(
                        "chat_pruner_catchup_startup",
                        t0.elapsed().as_millis() as u64,
                        true,
                        Some(format!("deleted_threads={n}")),
                    );
                    if n > 0 {
                        eprintln!(
                            "[chat-pruner] start: usunięto {n} nieaktywnych wątków czatu (>{} dni)",
                            chat_cleanup::CHAT_INACTIVITY_DAYS
                        );
                    }
                }
                Err(e) => {
                    wm.record(
                        "chat_pruner_catchup_startup",
                        t0.elapsed().as_millis() as u64,
                        false,
                        Some(e.to_string()),
                    );
                    eprintln!("[chat-pruner] start: błąd: {e}");
                }
            }
        });
    }

    // Stałe zadanie w tle — co kilka godzin przegląda i usuwa nieaktywne wątki.
    let _pruner_handle = chat_cleanup::spawn_chat_pruner_task(db.clone(), worker_metrics.clone());

    // Auto-składki dla zawodników z przelewem stałym — sprawdzaj raz dziennie i
    // dla bieżącego miesiąca twórz Approved-wpisy, jeśli ich brakuje. Catch-up
    // przy starcie (gdyby backend był wyłączony 1-go).
    {
        let db_for_initial = db.clone();
        let wm = worker_metrics.clone();
        tokio::spawn(async move {
            let t0 = std::time::Instant::now();
            match payments_scheduler::run_standing_orders_for_current_month(&db_for_initial).await {
                Ok(n) => {
                    wm.record(
                        "standing_order_catchup_startup",
                        t0.elapsed().as_millis() as u64,
                        true,
                        Some(format!("created_auto_payments={n}")),
                    );
                    if n > 0 {
                        eprintln!(
                            "[standing-order] start: utworzono {n} auto-składek za bieżący miesiąc."
                        );
                    }
                }
                Err(e) => {
                    wm.record(
                        "standing_order_catchup_startup",
                        t0.elapsed().as_millis() as u64,
                        false,
                        Some(e.to_string()),
                    );
                    eprintln!("[standing-order] start: błąd: {e}");
                }
            }
        });
    }
    let _standing_order_handle =
        payments_scheduler::spawn_standing_order_task(db.clone(), worker_metrics.clone());

    let state = AppState {
        db,
        jwt_secret,
        cloudinary_cloud_name,
        cloudinary_api_key,
        cloudinary_api_secret,
        worker_metrics,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Ok(router::build_router(state, cors))
}
