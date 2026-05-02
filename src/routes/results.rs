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
use crate::middleware::auth::{Claims, RequireTrainerOrHigher};
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
    _auth: RequireTrainerOrHigher,
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
        Role::Admin | Role::SuperAdmin | Role::TrainerAdmin => ResultStatus::Approved,
        Role::Trainer | Role::Athlete => ResultStatus::Pending,
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
    _auth: RequireTrainerOrHigher,
) -> Result<StatusCode, ApiError> {
    state.db.execute("UPDATE results SET status = 'Approved' WHERE id = ?1", [id])
        .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub struct UpdateResultRequest {
    pub snatch: Option<f64>,
    pub clean_and_jerk: Option<f64>,
    pub total: Option<f64>,
    pub date: Option<String>,
    pub status: Option<String>,
}

pub async fn list_all_results_staff(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date FROM results ORDER BY date DESC",
            (),
        )
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

pub async fn update_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
    Json(payload): Json<UpdateResultRequest>,
) -> Result<Json<CompetitionResult>, ApiError> {
    if payload.snatch.is_none()
        && payload.clean_and_jerk.is_none()
        && payload.total.is_none()
        && payload.date.is_none()
        && payload.status.is_none()
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "At least one field is required",
        ));
    }

    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date FROM results WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Result not found"))?;

    let mut cr = competition_result_from_row(&row)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if let Some(v) = payload.snatch {
        cr.snatch = v;
    }
    if let Some(v) = payload.clean_and_jerk {
        cr.clean_and_jerk = v;
    }
    if let Some(v) = payload.total {
        cr.total = v;
    } else if payload.snatch.is_some() || payload.clean_and_jerk.is_some() {
        cr.total = cr.snatch + cr.clean_and_jerk;
    }
    if let Some(d) = payload.date {
        cr.date = d;
    }
    if let Some(ref st) = payload.status {
        cr.status = st
            .parse::<ResultStatus>()
            .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    }

    state
        .db
        .execute(
            "UPDATE results SET snatch = ?1, clean_and_jerk = ?2, total = ?3, status = ?4, date = ?5 WHERE id = ?6",
            (
                cr.snatch,
                cr.clean_and_jerk,
                cr.total,
                cr.status.to_string(),
                cr.date.clone(),
                id,
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(cr))
}

pub async fn delete_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
) -> Result<StatusCode, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id FROM results WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(StatusCode::NOT_FOUND, "Result not found"));
    }

    state
        .db
        .execute("DELETE FROM results WHERE id = ?1", [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
