use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{SecondsFormat, Utc};
use libsql::Row;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::{Claims, claims_has_staff_access};
use crate::sql_row;
use crate::state::AppState;

/// Odczyt TEXT jako BLOB — omija panikę libsql przy uszkodzonym UTF-8 w SQLite TEXT.
const EXERCISES_SELECT_SQL: &str = "\
    SELECT id,
           CAST(name AS BLOB) AS name,
           CAST(category AS BLOB) AS category,
           CAST(description AS BLOB) AS description,
           CAST(video_url AS BLOB) AS video_url,
           CAST(created_at AS BLOB) AS created_at
    FROM exercises";

#[derive(Debug, Serialize)]
pub struct ExerciseBoardRow {
    pub athlete_id: String,
    pub athlete_name: String,
    pub squat_kg: Option<f64>,
    pub bench_kg: Option<f64>,
    pub deadlift_kg: Option<f64>,
    pub source_trainer_direct: bool,
    pub source_athlete_pending_count: i64,
    pub source_approved_results_count: i64,
    pub source_training_log_count: i64,
    pub source_last_approved_date: Option<String>,
}

pub async fn list_exercises_board(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Vec<ExerciseBoardRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT a.id, CAST(a.full_name AS BLOB), \
             MAX(r.squat_kg), MAX(r.bench_kg), MAX(r.deadlift_kg), \
             COUNT(CASE WHEN r.status = 'Pending' THEN 1 END) as pending_count, \
             COUNT(CASE WHEN r.status = 'Approved' THEN 1 END) as approved_count \
             FROM athletes a \
             LEFT JOIN results r ON a.id = r.athlete_id \
             GROUP BY a.id",
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
        out.push(ExerciseBoardRow {
            athlete_id: sql_row::required_lossy_string(&row, 0)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_name: sql_row::required_lossy_string(&row, 1)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            squat_kg: sql_row::opt_f64(&row, 2)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            bench_kg: sql_row::opt_f64(&row, 3)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            deadlift_kg: sql_row::opt_f64(&row, 4)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            source_trainer_direct: true,
            source_athlete_pending_count: sql_row::opt_i64(&row, 5)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .unwrap_or(0),
            source_approved_results_count: sql_row::opt_i64(&row, 6)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .unwrap_or(0),
            source_training_log_count: 0,
            source_last_approved_date: None,
        });
    }
    Ok(Json(out))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExerciseDto {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub video_url: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateExerciseRequest {
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub video_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateExerciseRequest {
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub video_url: Option<String>,
}

fn row_to_exercise_dto(row: &Row) -> Result<ExerciseDto, ApiError> {
    Ok(ExerciseDto {
        id: sql_row::required_lossy_string(row, 0)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        name: sql_row::required_lossy_string(row, 1)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        category: sql_row::lossy_opt_string(row, 2)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        description: sql_row::lossy_opt_string(row, 3)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        video_url: sql_row::lossy_opt_string(row, 4)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        created_at: sql_row::required_lossy_string(row, 5)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
    })
}

pub async fn list_exercises(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Vec<ExerciseDto>>, ApiError> {
    let sql = format!("{EXERCISES_SELECT_SQL} ORDER BY name ASC");
    let mut rows = state
        .db
        .query(&sql, ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(row_to_exercise_dto(&row)?);
    }
    Ok(Json(out))
}

pub async fn create_exercise(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<CreateExerciseRequest>,
) -> Result<Json<ExerciseDto>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień"));
    }
    if payload.name.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Name is required"));
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    state
        .db
        .execute(
            "INSERT INTO exercises (id, name, category, description, video_url, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id.clone(),
                payload.name.trim().to_string(),
                payload.category.clone(),
                payload.description.clone(),
                payload.video_url.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ExerciseDto {
        id,
        name: payload.name,
        category: payload.category,
        description: payload.description,
        video_url: payload.video_url,
        created_at: now,
    }))
}

pub async fn update_exercise(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
    Json(payload): Json<UpdateExerciseRequest>,
) -> Result<Json<ExerciseDto>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień"));
    }
    if payload.name.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Name is required"));
    }

    let sql = format!("{EXERCISES_SELECT_SQL} WHERE id = ?1");
    let mut rows = state
        .db
        .query(&sql, [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let existing = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if existing.is_none() {
        return Err(api_error(StatusCode::NOT_FOUND, "Exercise not found"));
    }

    state
        .db
        .execute(
            "UPDATE exercises SET name = ?1, category = ?2, description = ?3, video_url = ?4 WHERE id = ?5",
            (
                payload.name.trim().to_string(),
                payload.category.clone(),
                payload.description.clone(),
                payload.video_url.clone(),
                id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut rows = state
        .db
        .query(&sql, [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Exercise row missing"))?;

    Ok(Json(row_to_exercise_dto(&row)?))
}

pub async fn delete_exercise(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień"));
    }
    let n = state
        .db
        .execute("DELETE FROM exercises WHERE id = ?1", [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Exercise not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
