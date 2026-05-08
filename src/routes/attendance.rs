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

#[derive(Serialize)]
pub struct AttendanceSummary {
    pub athlete_id: String,
    pub present_count: i64,
    pub absent_count: i64,
    pub pending_count: i64,
    pub attendance_percent: f64,
}

#[derive(Deserialize)]
pub struct UpsertAttendanceRequest {
    pub athlete_id: String,
    pub session_date: String,
    pub status: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Deserialize)]
pub struct AttendanceListQuery {
    pub athlete_id: Option<String>,
    pub status: Option<String>,
    pub verification_state: Option<String>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
}

fn normalize_status(raw: &str) -> Result<String, ApiError> {
    let v = raw.trim().to_lowercase();
    match v.as_str() {
        "obecny" | "present" => Ok("obecny".to_string()),
        "spozniony" | "spóźniony" | "late" => Ok("nieobecny".to_string()),
        "nieobecny" | "absent" => Ok("nieobecny".to_string()),
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "status musi być: obecny | nieobecny",
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
        let athlete_label = notifications::athlete_display_for_notification(state.db.as_ref(), &payload.athlete_id)
            .await
            .unwrap_or_else(|_| "Zawodnik".to_string());
        notifications::notify_admin_broadcast(
            &state,
            "attendance_pending",
            "Nowe zgłoszenie obecności",
            &format!(
                "{} — obecność do weryfikacji ({}).",
                athlete_label,
                payload.session_date.trim()
            ),
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

pub async fn list_attendance(
    State(state): State<AppState>,
    claims: Claims,
    axum::extract::Query(query): axum::extract::Query<AttendanceListQuery>,
) -> Result<Json<Vec<AttendanceRecord>>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak dostępu"));
    }

    let mut sql = String::from(
        "SELECT id, athlete_id, session_date, status, source_role, created_by, verified_by, verification_state, note, created_at, updated_at
         FROM attendance_records WHERE 1=1",
    );
    let mut params: Vec<String> = Vec::new();

    if let Some(v) = query.athlete_id.as_ref() {
        if !v.trim().is_empty() {
            sql.push_str(" AND athlete_id = ?");
            params.push(v.trim().to_string());
        }
    }
    if let Some(v) = query.status.as_ref() {
        if !v.trim().is_empty() {
            sql.push_str(" AND status = ?");
            params.push(v.trim().to_string());
        }
    }
    if let Some(v) = query.verification_state.as_ref() {
        if !v.trim().is_empty() {
            sql.push_str(" AND verification_state = ?");
            params.push(v.trim().to_string());
        }
    }
    if let Some(v) = query.from_date.as_ref() {
        if !v.trim().is_empty() {
            sql.push_str(" AND session_date >= ?");
            params.push(v.trim().to_string());
        }
    }
    if let Some(v) = query.to_date.as_ref() {
        if !v.trim().is_empty() {
            sql.push_str(" AND session_date <= ?");
            params.push(v.trim().to_string());
        }
    }
    sql.push_str(" ORDER BY session_date DESC, created_at DESC LIMIT 500");

    let mut rows = state
        .db
        .query(&sql, params)
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

pub async fn attendance_summary_for_athlete(
    State(state): State<AppState>,
    claims: Claims,
    Path(athlete_id): Path<String>,
) -> Result<Json<AttendanceSummary>, ApiError> {
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
            "SELECT
                SUM(CASE WHEN status = 'obecny' THEN 1 ELSE 0 END) AS present_count,
                SUM(CASE WHEN status = 'nieobecny' THEN 1 ELSE 0 END) AS absent_count,
                SUM(CASE WHEN verification_state = 'pending' THEN 1 ELSE 0 END) AS pending_count
             FROM attendance_records
             WHERE athlete_id = ?1",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Brak danych obecności"))?;

    let present_count: i64 = row.get(0).unwrap_or(0);
    let absent_count: i64 = row.get(1).unwrap_or(0);
    let pending_count: i64 = row.get(2).unwrap_or(0);
    let denom = (present_count + absent_count) as f64;
    let attendance_percent = if denom > 0.0 {
        ((present_count as f64 / denom) * 1000.0).round() / 10.0
    } else {
        0.0
    };

    Ok(Json(AttendanceSummary {
        athlete_id,
        present_count,
        absent_count,
        pending_count,
        attendance_percent,
    }))
}
