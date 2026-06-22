//! Współdzielona logika HTTP — używana przez `main` (Axum/Tokio) i testy.

use std::path::PathBuf;

use axum::http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

pub mod audit;
pub mod chat_cleanup;
pub mod db;
pub mod db_migrations;
pub mod logging;
pub mod dto;
pub mod http_metrics;
pub mod middleware;
pub mod models;
pub mod notifications;
pub mod payments_scheduler;
pub mod worker_metrics;
pub mod repos;
pub mod router;
pub mod routes;
pub mod state;
mod theme_presets;

pub(crate) mod api_error;
pub mod cloudinary;
pub mod cms_github;
pub mod cms_sanitize;
mod external_calendar_sync;
mod ai_coach_monthly;
mod distributed_throttle;
mod login_throttle;
pub mod production_guards;
mod pagination;
mod post_throttle;
mod sinclair;
pub mod sql;
mod sql_row;
mod sql_util;

#[cfg(test)]
mod athlete_dashboard_acl_integration_test;
#[cfg(test)]
mod import_http_integration_test;

use state::AppState;
use state::Db;

/// Skąd brać bazę: lokalny SQLite, Turso embedded replica (domyślnie) lub czysty HTTP.
#[derive(Debug, Clone)]
pub enum DatabaseBackend {
    Local(PathBuf),
    Remote {
        url: String,
        auth_token: String,
        /// Lokalna kopia zsynchronizowana z Turso — szybkie odczyty. `None` = sam HTTP.
        replica_path: Option<PathBuf>,
    },
}

/// Buduje router Axum (libsql: SQLite lokalnie lub Turso zdalnie + JWT).
pub async fn create_app(
    database: DatabaseBackend,
    jwt_secret: String,
    cloudinary_cloud_name: String,
    cloudinary_api_key: String,
    cloudinary_api_secret: String,
    groq_api_key: String,
    groq_model: String,
) -> Result<axum::Router, Box<dyn std::error::Error + Send + Sync>> {
    let db = Db::open_with_migrations(database).await?;

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
                        tracing::info!(
                            deleted_threads = n,
                            inactivity_days = chat_cleanup::CHAT_INACTIVITY_DAYS,
                            "chat-pruner startup: usunięto nieaktywne wątki"
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
                    tracing::error!(error = %e, "chat-pruner startup: błąd");
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
                        tracing::info!(
                            created = n,
                            "standing-order startup: utworzono auto-składki za bieżący miesiąc"
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
                    tracing::error!(error = %e, "standing-order startup: błąd");
                }
            }
        });
    }
    let _standing_order_handle =
        payments_scheduler::spawn_standing_order_task(db.clone(), worker_metrics.clone());

    let prometheus_metrics_enabled = http_metrics::prometheus_metrics_enabled();
    if prometheus_metrics_enabled {
        tracing::info!("Prometheus: GET /metrics włączony (PROMETHEUS_METRICS)");
    }

    let state = AppState {
        db,
        jwt_secret,
        cloudinary_cloud_name,
        cloudinary_api_key,
        cloudinary_api_secret,
        groq_api_key,
        groq_model,
        worker_metrics,
        http_metrics: http_metrics::HttpMetrics::new(),
        prometheus_metrics_enabled,
    };

    // Przy starcie usuń stare wpisy rate limit (gdy `DISTRIBUTED_THROTTLE=1`).
    {
        let state_for_prune = state.clone();
        tokio::spawn(async move {
            distributed_throttle::prune_rate_limit_hits(&state_for_prune).await;
        });
    }

    Ok(router::build_router(state, build_cors_layer()))
}

/// Dozwolone originy CORS — lista po przecinku w `CORS_ALLOWED_ORIGINS` lub domyślna whitelist.
fn build_cors_layer() -> CorsLayer {
    const DEFAULT_ORIGINS: &str = concat!(
        "http://localhost:3000,",
        "http://127.0.0.1:3000,",
        "https://cksslavia.vercel.app,",
        "https://cksslavia-git-main-jakubgawron2s-projects.vercel.app"
    );

    let raw = std::env::var("CORS_ALLOWED_ORIGINS").unwrap_or_else(|_| DEFAULT_ORIGINS.to_string());
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();

    if origins.is_empty() {
        return CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
    }

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
}
