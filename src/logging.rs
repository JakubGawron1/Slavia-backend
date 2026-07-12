//! Inicjalizacja `tracing` — format Slavia na stdout.
//!
//! **Format (domyślnie):** `[LEVEL]:    {where}   (because)   {[fix]} extra | field=value …`
//!
//! **HF Spaces** (auto gdy `SPACE_ID` lub `SLAVIA_LOG_STYLE=compact`):
//! ```text
//! 2026-07-12 12:03:01 UTC ERR | http | GET /api/foo → 502 (420ms)
//!   cause:  HTTP request returned 5xx
//!   action: HF: retry after cold start; persist: check Turso sync and DB pool
//!   id:     abc-123
//! ```
//!
//! **RODO (SEC-12):** nie loguj treści wiadomości czatu, promptów Trenera AI ani base64 załączników.

mod format;

pub use format::{file_name, format_line, log_style, LogStyle, SlaviaEventFormatter};

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Domyślny poziom: info dla aplikacji; tower-http wyciszony (access log w middleware).
const DEFAULT_FILTER: &str = "slavia_backend=info,tower_http=off,info";

/// Inicjalizuje globalny subscriber (wywołaj raz na starcie w `main`).
pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| {
            EnvFilter::try_new(
                std::env::var("SLAVIA_LOG")
                    .ok()
                    .as_deref()
                    .unwrap_or(DEFAULT_FILTER),
            )
        })
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_level(false)
                .event_format(SlaviaEventFormatter::new()),
        )
        .with(filter)
        .init();
}
