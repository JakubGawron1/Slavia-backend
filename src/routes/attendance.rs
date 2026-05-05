use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use crate::audit::write_audit_log;
use crate::middleware::auth::{claims_has_staff_access, Claims};
use crate::notifications;
use crate::state::AppState;

#[derive(Serialize)]
pub struct AttendanceRecord {
    pub id: String,
    pub athlete_id: String,
    pub session_date: String,
    pub status: String,
    pub source_role: String,
    pub created_by: Option<String>,
    pub verified_by: Option<String>,
    pub verification_state: String,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct UpsertAttendanceRequest {
    pub athlete_id: String,
    pub session_date: String,
    pub status: String,
    #[serde(default)]
    pub note: Option<String>,
}

fn normalize_status(raw: &str) -> Result<String, ApiError> {
    let v = raw.trim().to_lowercase();
    match v.as_str() {
        "obecny" | "present" => Ok("obecny".to_string()),
        "spozniony" | "spóźniony" | "late" => Ok("spóźniony".to_string()),
        "nieobecny" | "absent" => Ok("nieobecny".to_string()),
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "status musi być: obecny | spóźniony | nieobecny",
        )),
    }
}

fn actor_role_label(claims: &Claims) -> String {
    if claims.roles.iter().any(|r| matches!(r, crate::models::Role::SuperAdmin)) {
        return "superadmin".to_string();
    }
    if claims.roles.iter().any(|r| matches!(r, crate::models::Role::Admin)) {
        return "admin".to_string();
    }
    if claims.roles.iter().any(|r| matches!(r, crate::models::Role::Trainer)) {
        return "trainer".to_string();
    }
    "athlete".to_string()
}

pub async fn upsert_attendance(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<UpsertAttendanceRequest>,
) -> Result<Json<AttendanceRecord>, ApiError> {
    let status = normalize_status(&payload.status)?;
    let actor_role = actor_role_label(&claims);
    let is_staff = claims_has_staff_access(&claims);
    let verification_state = if is_staff { "verified" } else { "pending" };
    let created_by = claims.sub.clone();
    let verified_by = if is_staff { Some(claims.sub.clone()) } else { None };
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();

    state
        .db
        .execute(
            "INSERT INTO attendance_records (
                id, athlete_id, session_date, status, source_role, created_by, verified_by, verification_state, note, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            (
                id.clone(),
                payload.athlete_id.clone(),
                payload.session_date.clone(),
                status.clone(),
                actor_role.clone(),
                Some(created_by.clone()),
                verified_by.clone(),
                verification_state.to_string(),
                payload.note.clone(),
                now.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let _ = write_audit_log(
        state.db.as_ref(),
        Some(&claims.sub),
        Some(&actor_role),
        "attendance",
        "attendance_upsert",
        Some("athlete"),
        Some(&payload.athlete_id),
        Some(
            &serde_json::json!({
                "session_date": payload.session_date,
                "status": status,
                "verification_state": verification_state
            })
            .to_string(),
        ),
    )
    .await;

    if !is_staff {
        notifications::notify_admin_broadcast(
            &state,
            "attendance_pending",
            "Nowe zgłoszenie obecności",
            "Zawodnik zgłosił obecność wymagającą weryfikacji.",
            Some(
                serde_json::json!({
                    "athlete_id": payload.athlete_id,
                    "session_date": payload.session_date
                })
                .to_string(),
            ),
        );
    }

    Ok(Json(AttendanceRecord {
        id,
        athlete_id: payload.athlete_id,
        session_date: payload.session_date,
        status,
        source_role: actor_role,
        created_by: Some(created_by),
        verified_by,
        verification_state: verification_state.to_string(),
        note: payload.note,
        created_at: now.clone(),
        updated_at: now,
    }))
}

pub async fn list_attendance_for_athlete(
    State(state): State<AppState>,
    claims: Claims,
    Path(athlete_id): Path<String>,
) -> Result<Json<Vec<AttendanceRecord>>, ApiError> {
    if !claims_has_staff_access(&claims) {
        let mut rows = state
            .db
            .query(
                "SELECT id FROM athletes WHERE id = ?1 AND user_id = ?2",
                (athlete_id.clone(), claims.sub.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_none()
        {
            return Err(api_error(StatusCode::FORBIDDEN, "Brak dostępu do tej historii"));
        }
    }

    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, session_date, status, source_role, created_by, verified_by, verification_state, note, created_at, updated_at
             FROM attendance_records
             WHERE athlete_id = ?1
             ORDER BY session_date DESC, created_at DESC",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(AttendanceRecord {
            id: row.get(0).unwrap_or_default(),
            athlete_id: row.get(1).unwrap_or_default(),
            session_date: row.get(2).unwrap_or_default(),
            status: row.get(3).unwrap_or_default(),
            source_role: row.get(4).unwrap_or_default(),
            created_by: row.get(5).ok(),
            verified_by: row.get(6).ok(),
            verification_state: row.get(7).unwrap_or_else(|_| "verified".to_string()),
            note: row.get(8).ok(),
            created_at: row.get(9).unwrap_or_default(),
            updated_at: row.get(10).unwrap_or_default(),
        });
    }
    Ok(Json(out))
}
