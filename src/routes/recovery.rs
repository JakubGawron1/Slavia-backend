use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error, map_db_err};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, claims_has_staff_access};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct RecoveryLog {
    pub id: String,
    pub athlete_id: String,
    pub date: String,
    pub sleep_hours: f64,
    pub fatigue_level: i64,
    pub soreness_level: i64,
    pub readiness_level: i64,
    pub note: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct UpsertRecoveryRequest {
    pub date: String,
    pub sleep_hours: f64,
    pub fatigue_level: i64,
    pub soreness_level: i64,
    pub readiness_level: i64,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecoveryQuery {
    pub athlete_id: Option<String>,
}

async fn athlete_id_for_user(state: &AppState, user_id: &str) -> Result<Option<String>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id FROM athletes WHERE user_id = ?1",
            [user_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(row.and_then(|r| r.get::<String>(0).ok()))
}

fn row_to_log(row: &libsql::Row) -> RecoveryLog {
    RecoveryLog {
        id: row.get(0).unwrap_or_default(),
        athlete_id: row.get(1).unwrap_or_default(),
        date: row.get(2).unwrap_or_default(),
        sleep_hours: row.get(3).unwrap_or(0.0),
        fatigue_level: row.get(4).unwrap_or(0),
        soreness_level: row.get(5).unwrap_or(0),
        readiness_level: row.get(6).unwrap_or(0),
        note: row.get(7).ok(),
        created_at: row.get(8).unwrap_or_default(),
    }
}

fn actor_role_label(claims: &Claims) -> &'static str {
    if claims
        .roles
        .iter()
        .any(|r| matches!(r, crate::models::Role::SuperAdmin))
    {
        return "superadmin";
    }
    if claims
        .roles
        .iter()
        .any(|r| matches!(r, crate::models::Role::Admin))
    {
        return "admin";
    }
    if claims
        .roles
        .iter()
        .any(|r| matches!(r, crate::models::Role::Trainer))
    {
        return "trainer";
    }
    "athlete"
}

pub async fn upsert_my_recovery_log(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<UpsertRecoveryRequest>,
) -> Result<Json<RecoveryLog>, ApiError> {
    let athlete_id = athlete_id_for_user(&state, &claims.sub)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete profile not found"))?;

    if payload.date.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "date is required"));
    }
    let clamp = |v: i64| v.clamp(1, 10);
    let sleep = payload.sleep_hours.clamp(0.0, 24.0);
    let fatigue = clamp(payload.fatigue_level);
    let soreness = clamp(payload.soreness_level);
    let readiness = clamp(payload.readiness_level);
    let note = payload
        .note
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut exists = state
        .db
        .query(
            "SELECT id, created_at FROM recovery_logs WHERE athlete_id = ?1 AND date = ?2",
            (athlete_id.clone(), payload.date.trim().to_string()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    if let Some(row) = exists
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id: String = row.get(0).unwrap_or_default();
        state
            .db
            .execute(
                "UPDATE recovery_logs SET sleep_hours = ?1, fatigue_level = ?2, soreness_level = ?3, readiness_level = ?4, note = ?5 WHERE id = ?6",
                (sleep, fatigue, soreness, readiness, note.clone(), id.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let details = serde_json::json!({
            "date": payload.date.trim(),
            "sleep_hours": sleep,
            "fatigue_level": fatigue,
            "soreness_level": soreness,
            "readiness_level": readiness
        })
        .to_string();
        let conn_arc = state.db_conn().await?;
        let _ = write_audit_log(
            conn_arc.as_ref(),
            Some(&claims.sub),
            Some(actor_role_label(&claims)),
            "recovery",
            "update",
            Some("athlete"),
            Some(&athlete_id),
            Some(&details),
        )
        .await;
        return Ok(Json(RecoveryLog {
            id,
            athlete_id,
            date: payload.date.trim().to_string(),
            sleep_hours: sleep,
            fatigue_level: fatigue,
            soreness_level: soreness,
            readiness_level: readiness,
            note,
            created_at: row.get(1).unwrap_or(now),
        }));
    }

    let id = Uuid::new_v4().to_string();
    state
        .db
        .execute(
            "INSERT INTO recovery_logs (id, athlete_id, date, sleep_hours, fatigue_level, soreness_level, readiness_level, note, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                id.clone(),
                athlete_id.clone(),
                payload.date.trim().to_string(),
                sleep,
                fatigue,
                soreness,
                readiness,
                note.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| map_db_err(e, "Masz już wpis regeneracji na ten dzień."))?;

    let conn_arc = state.db_conn().await?;
    let athlete_label =
        crate::notifications::athlete_display_for_notification(conn_arc.as_ref(), &athlete_id)
            .await
            .unwrap_or_else(|_| "Zawodnik".to_string());

    crate::notifications::notify_admin_broadcast(
        &state,
        "recovery_checkin",
        "Nowy check-in regeneracji",
        &format!(
            "{} — dzienny check-in regeneracji ({}).",
            athlete_label,
            payload.date.trim()
        ),
        Some(
            serde_json::json!({ "athlete_id": athlete_id, "date": payload.date.trim() })
                .to_string(),
        ),
    );
    let details = serde_json::json!({
        "date": payload.date.trim(),
        "sleep_hours": sleep,
        "fatigue_level": fatigue,
        "soreness_level": soreness,
        "readiness_level": readiness
    })
    .to_string();
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&claims.sub),
        Some(actor_role_label(&claims)),
        "recovery",
        "create",
        Some("athlete"),
        Some(&athlete_id),
        Some(&details),
    )
    .await;

    Ok(Json(RecoveryLog {
        id,
        athlete_id,
        date: payload.date.trim().to_string(),
        sleep_hours: sleep,
        fatigue_level: fatigue,
        soreness_level: soreness,
        readiness_level: readiness,
        note,
        created_at: now,
    }))
}

pub async fn list_recovery_logs(
    State(state): State<AppState>,
    claims: Claims,
    Query(query): Query<RecoveryQuery>,
) -> Result<Json<Vec<RecoveryLog>>, ApiError> {
    let athlete_id = if claims_has_staff_access(&claims) {
        query.athlete_id.ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "athlete_id is required for staff view",
            )
        })?
    } else {
        athlete_id_for_user(&state, &claims.sub)
            .await?
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete profile not found"))?
    };

    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, date, sleep_hours, fatigue_level, soreness_level, readiness_level, note, created_at \
             FROM recovery_logs WHERE athlete_id = ?1 ORDER BY date DESC, created_at DESC LIMIT 120",
            [athlete_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(row_to_log(&row));
    }
    Ok(Json(out))
}

pub async fn list_recovery_logs_for_athlete(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    claims: Claims,
) -> Result<Json<Vec<RecoveryLog>>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }
    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, date, sleep_hours, fatigue_level, soreness_level, readiness_level, note, created_at \
             FROM recovery_logs WHERE athlete_id = ?1 ORDER BY date DESC, created_at DESC LIMIT 120",
            [athlete_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(row_to_log(&row));
    }
    Ok(Json(out))
}
