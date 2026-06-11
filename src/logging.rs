//! Inicjalizacja `tracing` — stdout z filtrem `RUST_LOG` / `SLAVIA_LOG`.
//!
//! **RODO (SEC-12):** nie loguj treści wiadomości czatu, promptów Trenera AI ani base64 załączników.
//! `TraceLayer` rejestruje wyłącznie metodę i ścieżkę (bez body); ścieżki `/api/ai/coach/*` bez query string.

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Domyślny poziom: info dla aplikacji i tower-http; szczegóły przez `RUST_LOG`.
const DEFAULT_FILTER: &str = "slavia_backend=info,tower_http=info,info";

/// Inicjalizuje globalny subscriber (wywołaj raz na starcie w `main`).
pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(
            std::env::var("SLAVIA_LOG")
                .ok()
                .as_deref()
                .unwrap_or(DEFAULT_FILTER),
        ))
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true).with_thread_ids(false))
        .with(filter)
        .init();
}
