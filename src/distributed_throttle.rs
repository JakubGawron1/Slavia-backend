//! Opcjonalny limiter oparty o SQLite/Turso — współdzielony między instancjami backendu.
//! Na Turso włączone domyślnie; lokalnie in-memory. Wymuś: `DISTRIBUTED_THROTTLE=1`; wyłącz: `=0`.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::api_error::{ApiError, api_error};
use crate::post_throttle::{self, AiCoachLimitDeny};
use crate::state::AppState;
use axum::http::StatusCode;

fn distributed_enabled() -> bool {
    if let Ok(v) = std::env::var("DISTRIBUTED_THROTTLE") {
        let t = v.trim().to_ascii_lowercase();
        if matches!(t.as_str(), "0" | "false" | "no" | "off") {
            return false;
        }
        if matches!(t.as_str(), "1" | "true" | "yes") {
            return true;
        }
    }
    // Turso / multi-instance: domyślnie SQLite-backed throttle (SEC-10).
    crate::production_guards::remote_database_configured()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn count_hits(
    state: &AppState,
    scope_key: &str,
    bucket: &str,
    window_ms: i64,
) -> Result<usize, ApiError> {
    let cutoff = now_ms() - window_ms;
    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM rate_limit_hits WHERE scope_key = ?1 AND bucket = ?2 AND hit_at_ms > ?3",
            (scope_key.to_string(), bucket.to_string(), cutoff),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let n: i64 = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(|r| r.get(0).unwrap_or(0))
        .unwrap_or(0);
    Ok(n as usize)
}

async fn record_hit(state: &AppState, scope_key: &str, bucket: &str) -> Result<(), ApiError> {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .execute(
            "INSERT INTO rate_limit_hits (id, scope_key, bucket, hit_at_ms) VALUES (?1, ?2, ?3, ?4)",
            (
                id,
                scope_key.to_string(),
                bucket.to_string(),
                now_ms(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

async fn reserve_buckets(
    state: &AppState,
    scope_key: &str,
    checks: &[(&str, u64, usize)],
) -> Result<(), ()> {
    for (bucket, window_secs, max) in checks {
        let window_ms = (*window_secs as i64) * 1000;
        let count = count_hits(state, scope_key, bucket, window_ms)
            .await
            .map_err(|_| ())?;
        if count >= *max {
            return Err(());
        }
    }
    for (bucket, _, _) in checks {
        record_hit(state, scope_key, bucket)
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

/// Kontakt — per IP; fallback do in-memory gdy wyłączone.
pub async fn reserve_contact_submit(state: &AppState, client_ip: &str) -> Result<(), ()> {
    if !distributed_enabled() {
        return post_throttle::reserve_contact_submit(client_ip);
    }
    let ip = client_ip.trim();
    if ip.is_empty() || ip == "unknown" {
        return Err(());
    }
    let scope = format!("ip::{ip}");
    reserve_buckets(
        state,
        &scope,
        &[
            ("contact_submit", 300, 5),
            ("contact_submit_daily", 86_400, 20),
        ],
    )
    .await
}

/// Publiczny czat AI — per IP + globalny klub; fallback in-memory.
pub async fn reserve_ai_coach_public_chat(
    state: &AppState,
    client_ip: &str,
) -> Result<(), AiCoachLimitDeny> {
    if !distributed_enabled() {
        return post_throttle::reserve_ai_coach_public_chat(client_ip);
    }
    let ip = client_ip.trim();
    if ip.is_empty() || ip == "unknown" {
        return Err(AiCoachLimitDeny::ChatMinute);
    }
    let scope = format!("ip::{ip}");
    if reserve_buckets(
        state,
        &scope,
        &[
            ("ai_coach_public_chat", 60, 3),
            ("ai_coach_public_chat_daily", 86_400, 25),
        ],
    )
    .await
    .is_err()
    {
        return Err(AiCoachLimitDeny::ChatMinute);
    }
    if reserve_buckets(
        state,
        post_throttle::AI_COACH_CLUB_GLOBAL_SUB,
        &[
            ("ai_coach_club_global_chat", 60, 8),
            ("ai_coach_club_global_chat_daily", 86_400, 300),
        ],
    )
    .await
    .is_err()
    {
        return Err(AiCoachLimitDeny::ClubChatMinute);
    }
    Ok(())
}

/// Usuwa stare wpisy (okno dobowe + bufor).
pub async fn prune_rate_limit_hits(state: &AppState) {
    if !distributed_enabled() {
        return;
    }
    let cutoff = now_ms() - (86_400_i64 * 2 * 1000);
    let _ = state
        .db
        .execute(
            "DELETE FROM rate_limit_hits WHERE hit_at_ms < ?1",
            [cutoff],
        )
        .await;
}
