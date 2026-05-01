use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use libsql::Row;

use crate::api_error::{api_error, ApiError};
use crate::state::AppState;
use crate::models::{CompetitionResult, ResultStatus, Role};
use crate::middleware::auth::{Claims, RequireAdminOrSuperAdmin};
use crate::sql_row;

fn competition_result_from_row(row: &Row) -> Result<CompetitionResult, String> {
    let status_str = sql_row::required_string(row, 5)?;
    let status = status_str
        .parse::<ResultStatus>()
        .map_err(|e| format!("{} (status={:?})", e, status_str))?;
    Ok(CompetitionResult {
        id: sql_row::required_string(row, 0)?,
        athlete_id: sql_row::required_string(row, 1)?,
        snatch: sql_row::required_f64(row, 2)?,
        clean_and_jerk: sql_row::required_f64(row, 3)?,
        total: sql_row::required_f64(row, 4)?,
        status,
        date: sql_row::required_string(row, 6)?,
    })
}

#[derive(Deserialize)]
pub struct CreateResultRequest {
    pub athlete_id: String,
    pub snatch: f64,
    pub clean_and_jerk: f64,
    pub total: f64,
    pub date: String,
}

pub async fn list_approved_results(
    State(state): State<AppState>,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date FROM results WHERE status = 'Approved'", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

pub async fn list_pending_results(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date FROM results WHERE status = 'Pending'", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

pub async fn list_athlete_results(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date FROM results WHERE athlete_id = ?1", [athlete_id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

pub async fn create_result(
    State(state): State<AppState>,
    claims: Claims, // Must be authenticated
    Json(payload): Json<CreateResultRequest>,
) -> Result<Json<CompetitionResult>, ApiError> {
    let status = match claims.role {
        Role::Admin | Role::SuperAdmin => ResultStatus::Approved,
        Role::Athlete => ResultStatus::Pending,
    };

    if claims.role == Role::Athlete {
        let mut rows = state.db.query("SELECT id FROM athletes WHERE user_id = ?1", [claims.sub])
            .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
            let athlete_id = sql_row::required_string(&row, 0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            if athlete_id != payload.athlete_id {
                return Err(api_error(StatusCode::FORBIDDEN, "Athletes can only submit their own results"));
            }
        } else {
            return Err(api_error(StatusCode::NOT_FOUND, "Athlete profile not found"));
        }
    }

    let id = Uuid::new_v4().to_string();
    
    state.db.execute(
        "INSERT INTO results (id, athlete_id, snatch, clean_and_jerk, total, status, date) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (id.clone(), payload.athlete_id.clone(), payload.snatch, payload.clean_and_jerk, payload.total, status.to_string(), payload.date.clone()),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(CompetitionResult {
        id,
        athlete_id: payload.athlete_id,
        snatch: payload.snatch,
        clean_and_jerk: payload.clean_and_jerk,
        total: payload.total,
        status,
        date: payload.date,
    }))
}

pub async fn approve_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, ApiError> {
    state.db.execute("UPDATE results SET status = 'Approved' WHERE id = ?1", [id])
        .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}
