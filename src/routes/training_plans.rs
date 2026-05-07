use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use crate::audit::write_audit_log;
use crate::middleware::auth::{claims_has_staff_access, Claims};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct TrainingPlanDto {
    pub id: String,
    pub athlete_id: String,
    pub title: String,
    pub goal: Option<String>,
    pub week_start: String,
    pub status: String,
    pub coach_note: Option<String>,
    pub athlete_note: Option<String>,
    pub progress_percent: i64,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTrainingPlanRequest {
    pub athlete_id: String,
    pub title: String,
    pub goal: Option<String>,
    pub week_start: String,
    pub status: Option<String>,
    pub coach_note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTrainingPlanRequest {
    pub title: Option<String>,
    pub goal: Option<String>,
    pub week_start: Option<String>,
    pub status: Option<String>,
    pub coach_note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMyProgressRequest {
    pub status: Option<String>,
    pub athlete_note: Option<String>,
    pub progress_percent: Option<i64>,
}

fn normalize_status(s: Option<&str>) -> String {
    let raw = s.unwrap_or("planned").trim().to_lowercase();
    if matches!(raw.as_str(), "planned" | "active" | "completed" | "paused") {
        raw
    } else {
        "planned".to_string()
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

fn row_to_dto(row: &libsql::Row) -> TrainingPlanDto {
    TrainingPlanDto {
        id: row.get(0).unwrap(),
        athlete_id: row.get(1).unwrap(),
        title: row.get(2).unwrap(),
        goal: row.get(3).ok(),
        week_start: row.get(4).unwrap(),
        status: row.get(5).unwrap(),
        coach_note: row.get(6).ok(),
        athlete_note: row.get(7).ok(),
        progress_percent: row.get(8).unwrap_or(0),
        created_by: row.get(9).ok(),
        created_at: row.get(10).unwrap(),
        updated_at: row.get(11).unwrap(),
    }
}

async fn athlete_id_for_user(state: &AppState, user_id: &str) -> Result<Option<String>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id FROM athletes WHERE user_id = ?1", [user_id.to_string()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(row.and_then(|r| r.get::<String>(0).ok()))
}

pub async fn list_my_training_plans(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Vec<TrainingPlanDto>>, ApiError> {
    let athlete_id = athlete_id_for_user(&state, &claims.sub)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete profile not found"))?;

    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, title, goal, week_start, status, coach_note, athlete_note, progress_percent, created_by, created_at, updated_at \
             FROM training_plans WHERE athlete_id = ?1 ORDER BY week_start DESC, updated_at DESC",
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
        out.push(row_to_dto(&row));
    }
    Ok(Json(out))
}

pub async fn list_athlete_training_plans(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    claims: Claims,
) -> Result<Json<Vec<TrainingPlanDto>>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }
    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, title, goal, week_start, status, coach_note, athlete_note, progress_percent, created_by, created_at, updated_at \
             FROM training_plans WHERE athlete_id = ?1 ORDER BY week_start DESC, updated_at DESC",
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
        out.push(row_to_dto(&row));
    }
    Ok(Json(out))
}

pub async fn create_training_plan(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<CreateTrainingPlanRequest>,
) -> Result<Json<TrainingPlanDto>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }
    if payload.title.trim().is_empty() || payload.week_start.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "title and week_start are required"));
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let status = normalize_status(payload.status.as_deref());
    let goal = payload.goal.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let coach_note = payload
        .coach_note
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    state
        .db
        .execute(
            "INSERT INTO training_plans (id, athlete_id, title, goal, week_start, status, coach_note, progress_percent, created_by, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?9)",
            (
                id.clone(),
                payload.athlete_id.clone(),
                payload.title.trim().to_string(),
                goal.clone(),
                payload.week_start.trim().to_string(),
                status.clone(),
                coach_note.clone(),
                claims.sub.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    crate::notifications::notify_training_plan_assigned(
        &state,
        &payload.athlete_id,
        payload.title.trim(),
        payload.week_start.trim(),
    );
    let details = serde_json::json!({
        "title": payload.title.trim(),
        "week_start": payload.week_start.trim(),
        "status": status
    })
    .to_string();
    let _ = write_audit_log(
        state.db.as_ref(),
        Some(&claims.sub),
        Some(actor_role_label(&claims)),
        "training_plan",
        "create",
        Some("athlete"),
        Some(&payload.athlete_id),
        Some(&details),
    )
    .await;

    Ok(Json(TrainingPlanDto {
        id,
        athlete_id: payload.athlete_id,
        title: payload.title.trim().to_string(),
        goal,
        week_start: payload.week_start.trim().to_string(),
        status,
        coach_note,
        athlete_note: None,
        progress_percent: 0,
        created_by: Some(claims.sub),
        created_at: now.clone(),
        updated_at: now,
    }))
}

pub async fn update_training_plan(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
    claims: Claims,
    Json(payload): Json<UpdateTrainingPlanRequest>,
) -> Result<StatusCode, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let status = payload.status.as_deref().map(|s| normalize_status(Some(s)));
    state
        .db
        .execute(
            "UPDATE training_plans SET
               title = COALESCE(?1, title),
               goal = COALESCE(?2, goal),
               week_start = COALESCE(?3, week_start),
               status = COALESCE(?4, status),
               coach_note = COALESCE(?5, coach_note),
               updated_at = ?6
             WHERE id = ?7",
            (
                payload.title.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                payload.goal.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                payload.week_start.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                status,
                payload.coach_note.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                now,
                plan_id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let details = serde_json::json!({
        "plan_id": plan_id,
        "title": payload.title,
        "week_start": payload.week_start,
        "status": payload.status
    })
    .to_string();
    let _ = write_audit_log(
        state.db.as_ref(),
        Some(&claims.sub),
        Some(actor_role_label(&claims)),
        "training_plan",
        "update",
        Some("training_plan"),
        Some(&plan_id),
        Some(&details),
    )
    .await;
    Ok(StatusCode::OK)
}

pub async fn update_my_plan_progress(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
    claims: Claims,
    Json(payload): Json<UpdateMyProgressRequest>,
) -> Result<StatusCode, ApiError> {
    let my_athlete_id = athlete_id_for_user(&state, &claims.sub)
        .await?
        .ok_or_else(|| api_error(StatusCode::FORBIDDEN, "Brak profilu zawodnika"))?;

    let mut rows = state
        .db
        .query(
            "SELECT athlete_id, title FROM training_plans WHERE id = ?1",
            [plan_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Plan not found"))?;
    let owner_athlete_id: String = row.get(0).unwrap_or_default();
    let title: String = row.get(1).unwrap_or_else(|_| "Plan treningowy".to_string());
    if owner_athlete_id != my_athlete_id {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak dostępu do planu"));
    }

    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let status = payload.status.as_deref().map(|s| normalize_status(Some(s)));
    let progress = payload.progress_percent.map(|p| p.clamp(0, 100));
    let athlete_note = payload
        .athlete_note
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    state
        .db
        .execute(
            "UPDATE training_plans SET
               status = COALESCE(?1, status),
               athlete_note = COALESCE(?2, athlete_note),
               progress_percent = COALESCE(?3, progress_percent),
               updated_at = ?4
             WHERE id = ?5",
            (status, athlete_note, progress, now, plan_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    crate::notifications::notify_training_plan_progress_updated(
        &state,
        &my_athlete_id,
        &title,
        payload.progress_percent.unwrap_or(0).clamp(0, 100),
    );
    let details = serde_json::json!({
        "plan_id": plan_id,
        "status": payload.status,
        "progress_percent": payload.progress_percent,
        "athlete_note": payload.athlete_note
    })
    .to_string();
    let _ = write_audit_log(
        state.db.as_ref(),
        Some(&claims.sub),
        Some(actor_role_label(&claims)),
        "training_plan",
        "athlete_progress_update",
        Some("training_plan"),
        Some(&plan_id),
        Some(&details),
    )
    .await;
    Ok(StatusCode::OK)
}

pub async fn delete_training_plan(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
    claims: Claims,
) -> Result<StatusCode, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }
    let n = state
        .db
        .execute("DELETE FROM training_plans WHERE id = ?1", [plan_id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Plan not found"));
    }
    let _ = write_audit_log(
        state.db.as_ref(),
        Some(&claims.sub),
        Some(actor_role_label(&claims)),
        "training_plan",
        "delete",
        Some("training_plan"),
        Some(&plan_id),
        None,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
