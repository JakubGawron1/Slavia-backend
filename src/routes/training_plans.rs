use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, claims_has_staff_access};
use crate::sql_row;
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TrainingPlanItemDto {
    pub id: String,
    pub plan_id: String,
    pub week_number: i32,
    pub day_of_week: i32,
    pub exercise_id: Option<String>,
    pub custom_exercise_name: Option<String>,
    pub sets: Option<i32>,
    pub reps: Option<i32>,
    pub intensity_percent: Option<f64>,
    pub weight_kg: Option<f64>,
    pub notes: Option<String>,
    pub sort_order: i32,
    pub exercise_name: Option<String>, // z JOINa
}

#[derive(Debug, Deserialize)]
pub struct PlanItemPayload {
    pub week_number: Option<i32>,
    pub day_of_week: i32,
    pub exercise_id: Option<String>,
    pub custom_exercise_name: Option<String>,
    pub sets: Option<i32>,
    pub reps: Option<i32>,
    pub intensity_percent: Option<f64>,
    pub weight_kg: Option<f64>,
    pub notes: Option<String>,
    pub sort_order: i32,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePlanItemsRequest {
    pub items: Vec<PlanItemPayload>,
}

#[derive(Debug, Serialize)]
pub struct TrainingPlanDto {
    pub id: String,
    pub athlete_id: String,
    pub title: String,
    pub goal: Option<String>,
    pub week_start: String,
    pub duration_weeks: i64,
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
    pub duration_weeks: Option<i64>,
    pub status: Option<String>,
    pub coach_note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTrainingPlanRequest {
    pub title: Option<String>,
    pub goal: Option<String>,
    pub week_start: Option<String>,
    pub duration_weeks: Option<i64>,
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

fn clamp_duration_weeks(n: i64) -> i64 {
    n.clamp(1, 52)
}

fn clamp_week_number(n: i32) -> i32 {
    n.clamp(1, 52)
}

fn clamp_day_of_week(d: i32) -> i32 {
    d.clamp(1, 7)
}

fn row_to_dto(row: &libsql::Row) -> Result<TrainingPlanDto, libsql::Error> {
    Ok(TrainingPlanDto {
        id: row.get(0)?,
        athlete_id: row.get(1)?,
        title: row.get(2)?,
        goal: row.get(3).ok(),
        week_start: row.get(4)?,
        duration_weeks: row.get(5).unwrap_or(1),
        status: row.get(6)?,
        coach_note: row.get(7).ok(),
        athlete_note: row.get(8).ok(),
        progress_percent: row.get(9).unwrap_or(0),
        created_by: row.get(10).ok(),
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn row_to_plan_item_dto(row: &libsql::Row) -> Result<TrainingPlanItemDto, ApiError> {
    Ok(TrainingPlanItemDto {
        id: row.get(0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        plan_id: row.get(1).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        week_number: row.get(2).unwrap_or(1),
        day_of_week: row.get(3).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        exercise_id: row.get(4).ok(),
        custom_exercise_name: row.get(5).ok(),
        sets: row.get(6).ok(),
        reps: row.get(7).ok(),
        intensity_percent: row.get(8).ok(),
        weight_kg: row.get(9).ok(),
        notes: row.get(10).ok(),
        sort_order: row.get(11).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        exercise_name: sql_row::lossy_opt_string(row, 12)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    })
}

const PLAN_SELECT: &str = "SELECT id, athlete_id, title, goal, week_start, duration_weeks, status, coach_note, athlete_note, progress_percent, created_by, created_at, updated_at \
             FROM training_plans";

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
            &format!("{PLAN_SELECT} WHERE athlete_id = ?1 ORDER BY week_start DESC, updated_at DESC"),
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
        out.push(row_to_dto(&row).map_err(|e| {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?);
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
            &format!("{PLAN_SELECT} WHERE athlete_id = ?1 ORDER BY week_start DESC, updated_at DESC"),
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
        out.push(row_to_dto(&row).map_err(|e| {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?);
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
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "title and week_start are required",
        ));
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let status = normalize_status(payload.status.as_deref());
    let duration_weeks = clamp_duration_weeks(payload.duration_weeks.unwrap_or(1));
    let goal = payload
        .goal
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let coach_note = payload
        .coach_note
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    state
        .db
        .execute(
            "INSERT INTO training_plans (id, athlete_id, title, goal, week_start, duration_weeks, status, coach_note, progress_percent, created_by, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10, ?10)",
            (
                id.clone(),
                payload.athlete_id.clone(),
                payload.title.trim().to_string(),
                goal.clone(),
                payload.week_start.trim().to_string(),
                duration_weeks,
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
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
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
        duration_weeks,
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
    let duration_weeks = payload.duration_weeks.map(clamp_duration_weeks);
    state
        .db
        .execute(
            "UPDATE training_plans SET
               title = COALESCE(?1, title),
               goal = COALESCE(?2, goal),
               week_start = COALESCE(?3, week_start),
               duration_weeks = COALESCE(?4, duration_weeks),
               status = COALESCE(?5, status),
               coach_note = COALESCE(?6, coach_note),
               updated_at = ?7
             WHERE id = ?8",
            (
                payload
                    .title
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                payload
                    .goal
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                payload
                    .week_start
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                duration_weeks,
                status,
                payload
                    .coach_note
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
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
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
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
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
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
        .execute(
            "DELETE FROM training_plans WHERE id = ?1",
            [plan_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Plan not found"));
    }
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
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

pub async fn list_plan_items(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
    claims: Claims,
) -> Result<Json<Vec<TrainingPlanItemDto>>, ApiError> {
    // Bez tego checka każdy zalogowany mógłby odczytać plan po samym ID.
    // Zawodnik może czytać tylko własne plany; kadra (staff) może czytać wszystkie.
    if !claims_has_staff_access(&claims) {
        let my_athlete_id = athlete_id_for_user(&state, &claims.sub)
            .await?
            .ok_or_else(|| api_error(StatusCode::FORBIDDEN, "Brak profilu zawodnika"))?;

        let mut rows = state
            .db
            .query(
                "SELECT athlete_id FROM training_plans WHERE id = ?1",
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
        if owner_athlete_id != my_athlete_id {
            return Err(api_error(StatusCode::FORBIDDEN, "Brak dostępu do planu"));
        }
    }

    let mut rows = state
        .db
        .query(
            "SELECT i.id, i.plan_id, i.week_number, i.day_of_week, i.exercise_id, i.custom_exercise_name, i.sets, i.reps, i.intensity_percent, i.weight_kg, i.notes, i.sort_order, CAST(e.name AS BLOB) \
             FROM training_plan_items i \
             LEFT JOIN exercises e ON i.exercise_id = e.id \
             WHERE i.plan_id = ?1 ORDER BY i.week_number ASC, i.day_of_week ASC, i.sort_order ASC",
            [plan_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(row_to_plan_item_dto(&row)?);
    }
    Ok(Json(out))
}

pub async fn update_plan_items(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
    claims: Claims,
    Json(payload): Json<UpdatePlanItemsRequest>,
) -> Result<StatusCode, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień"));
    }

    // Upewnij się, że plan istnieje (czytelniejszy błąd niż ciche zapisanie pustej listy).
    let mut plan_rows = state
        .db
        .query(
            "SELECT athlete_id, title FROM training_plans WHERE id = ?1",
            [plan_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let plan_row = plan_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Plan not found"))?;
    let athlete_id: String = plan_row.get(0).unwrap_or_default();
    let plan_title: String = plan_row
        .get(1)
        .unwrap_or_else(|_| "Plan treningowy".to_string());

    let mut duration_rows = state
        .db
        .query(
            "SELECT duration_weeks FROM training_plans WHERE id = ?1",
            [plan_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let duration_row = duration_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let duration_weeks = duration_row
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(1);

    state
        .db
        .execute(
            "DELETE FROM training_plan_items WHERE plan_id = ?1",
            [plan_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    let items_count = payload.items.len();
    for item in payload.items {
        let week_number = clamp_week_number(item.week_number.unwrap_or(1));
        if i64::from(week_number) > duration_weeks {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "week_number {week_number} exceeds plan duration ({duration_weeks} weeks)"
                ),
            ));
        }
        let id = Uuid::new_v4().to_string();
        state.db.execute(
            "INSERT INTO training_plan_items (id, plan_id, week_number, day_of_week, exercise_id, custom_exercise_name, sets, reps, intensity_percent, weight_kg, notes, sort_order, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            (
                id,
                plan_id.clone(),
                week_number,
                clamp_day_of_week(item.day_of_week),
                item.exercise_id,
                item.custom_exercise_name,
                item.sets,
                item.reps,
                item.intensity_percent,
                item.weight_kg,
                item.notes,
                item.sort_order,
                now.clone(),
            )
        ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let details = serde_json::json!({
        "plan_id": plan_id,
        "title": plan_title,
        "items_count": items_count
    })
    .to_string();
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&claims.sub),
        Some(actor_role_label(&claims)),
        "training_plan_items",
        "update",
        Some("athlete"),
        Some(&athlete_id),
        Some(&details),
    )
    .await;

    Ok(StatusCode::OK)
}
