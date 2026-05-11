use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::{Claims, claims_has_staff_access};
use crate::state::AppState;

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
    // Prosta wersja: wyciągamy najlepsze Squat/Bench/Deadlift z tabeli results (Approved) dla każdego zawodnika.
    let mut rows = state
        .db
        .query(
            "SELECT a.id, a.full_name, \
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
            athlete_id: row.get(0).unwrap(),
            athlete_name: row.get(1).unwrap(),
            squat_kg: row.get(2).ok(),
            bench_kg: row.get(3).ok(),
            deadlift_kg: row.get(4).ok(),
            source_trainer_direct: true, // placeholder
            source_athlete_pending_count: row.get(5).unwrap_or(0),
            source_approved_results_count: row.get(6).unwrap_or(0),
            source_training_log_count: 0, // placeholder
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

pub async fn list_exercises(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Vec<ExerciseDto>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, name, category, description, video_url, created_at FROM exercises ORDER BY name ASC", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(ExerciseDto {
            id: row.get(0).unwrap(),
            name: row.get(1).unwrap(),
            category: row.get(2).ok(),
            description: row.get(3).ok(),
            video_url: row.get(4).ok(),
            created_at: row.get(5).unwrap(),
        });
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

pub async fn delete_exercise(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień"));
    }
    state
        .db
        .execute("DELETE FROM exercises WHERE id = ?1", [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
