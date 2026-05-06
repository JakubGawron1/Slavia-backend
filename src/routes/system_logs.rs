use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::{RequireSuperAdmin, RequireTrainerOrHigher};
use crate::state::AppState;

#[derive(Serialize)]
pub struct AuditLogRow {
    pub id: String,
    pub actor_user_id: Option<String>,
    pub actor_role: Option<String>,
    pub category: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub details: Option<String>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct SystemMetricsDto {
    pub athletes_count: i64,
    pub active_plans_count: i64,
    pub pending_results_count: i64,
    pub unread_notifications_count: i64,
    pub recovery_checkins_7d_count: i64,
    pub recent_events: Vec<AuditLogRow>,
}

#[derive(Serialize)]
pub struct OpsEventRow {
    pub source: String,
    pub at: String,
    pub title: String,
    pub detail: String,
}

pub async fn list_audit_logs(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
) -> Result<Json<Vec<AuditLogRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, actor_user_id, actor_role, category, action, target_type, target_id, details, created_at
             FROM system_audit_logs
             ORDER BY created_at DESC
             LIMIT 300",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(AuditLogRow {
            id: row.get(0).unwrap_or_default(),
            actor_user_id: row.get(1).ok(),
            actor_role: row.get(2).ok(),
            category: row.get(3).unwrap_or_else(|_| "system".to_string()),
            action: row.get(4).unwrap_or_else(|_| "unknown".to_string()),
            target_type: row.get(5).ok(),
            target_id: row.get(6).ok(),
            details: row.get(7).ok(),
            created_at: row.get(8).unwrap_or_default(),
        });
    }
    Ok(Json(out))
}

pub async fn system_metrics(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<SystemMetricsDto>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM athletes WHERE is_active IS NULL OR is_active = 1",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let athletes_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM training_plans WHERE status IN ('planned','active')",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let active_plans_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query("SELECT COUNT(*) FROM results WHERE status = 'Pending'", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let pending_results_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query("SELECT COUNT(*) FROM notifications WHERE is_read = 0", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let unread_notifications_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM recovery_logs WHERE date >= date('now', '-7 day')",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let recovery_checkins_7d_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query(
            "SELECT id, actor_user_id, actor_role, category, action, target_type, target_id, details, created_at
             FROM system_audit_logs
             ORDER BY created_at DESC
             LIMIT 20",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut recent_events = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        recent_events.push(AuditLogRow {
            id: row.get(0).unwrap_or_default(),
            actor_user_id: row.get(1).ok(),
            actor_role: row.get(2).ok(),
            category: row.get(3).unwrap_or_else(|_| "system".to_string()),
            action: row.get(4).unwrap_or_else(|_| "unknown".to_string()),
            target_type: row.get(5).ok(),
            target_id: row.get(6).ok(),
            details: row.get(7).ok(),
            created_at: row.get(8).unwrap_or_default(),
        });
    }

    Ok(Json(SystemMetricsDto {
        athletes_count,
        active_plans_count,
        pending_results_count,
        unread_notifications_count,
        recovery_checkins_7d_count,
        recent_events,
    }))
}

pub async fn event_feed(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<OpsEventRow>>, ApiError> {
    let mut out: Vec<OpsEventRow> = Vec::new();

    let mut rows = state
        .db
        .query(
            "SELECT date, athlete_id, total, status FROM results ORDER BY date DESC LIMIT 40",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let at: String = row.get(0).unwrap_or_default();
        let athlete_id: String = row.get(1).unwrap_or_default();
        let total: f64 = row.get(2).unwrap_or(0.0);
        let status: String = row.get(3).unwrap_or_default();
        out.push(OpsEventRow {
            source: "results".to_string(),
            at: at.clone(),
            title: format!("Wynik {} kg ({})", total, status),
            detail: format!("athlete_id={}", athlete_id),
        });
    }

    let mut rows = state
        .db
        .query(
            "SELECT session_date, athlete_id, status, verification_state FROM attendance_records ORDER BY session_date DESC LIMIT 40",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let at: String = row.get(0).unwrap_or_default();
        let athlete_id: String = row.get(1).unwrap_or_default();
        let status: String = row.get(2).unwrap_or_default();
        let verification_state: String = row.get(3).unwrap_or_default();
        out.push(OpsEventRow {
            source: "attendance".to_string(),
            at: at.clone(),
            title: format!("Obecność: {} ({})", status, verification_state),
            detail: format!("athlete_id={}", athlete_id),
        });
    }

    let mut rows = state
        .db
        .query(
            "SELECT date, athlete_id, sleep_hours, readiness_level FROM recovery_logs ORDER BY date DESC LIMIT 40",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let at: String = row.get(0).unwrap_or_default();
        let athlete_id: String = row.get(1).unwrap_or_default();
        let sleep_hours: f64 = row.get(2).unwrap_or(0.0);
        let readiness_level: i64 = row.get(3).unwrap_or(0);
        out.push(OpsEventRow {
            source: "recovery".to_string(),
            at: at.clone(),
            title: format!("Regeneracja: sen {}h, gotowość {}/10", sleep_hours, readiness_level),
            detail: format!("athlete_id={}", athlete_id),
        });
    }

    out.sort_by(|a, b| b.at.cmp(&a.at));
    out.truncate(120);
    Ok(Json(out))
}
