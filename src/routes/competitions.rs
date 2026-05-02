use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use crate::state::AppState;
use crate::models::Competition;
use crate::middleware::auth::RequireTrainerOrHigher;

#[derive(Deserialize)]
pub struct CreateCompetitionRequest {
    pub title: String,
    pub date: String,
    pub location: String,
    pub description: Option<String>,
    pub category: Option<String>, // "championship", "league", "club_event", "training"
    pub status: Option<String>, // "scheduled", "cancelled", "moved"
}

pub async fn list_competitions(
    State(state): State<AppState>,
) -> Result<Json<Vec<Competition>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, title, date, location, description, category, status FROM competitions ORDER BY date ASC", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut competitions = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        competitions.push(Competition {
            id: row.get(0).unwrap(),
            title: row.get(1).unwrap(),
            date: row.get(2).unwrap(),
            location: row.get(3).unwrap(),
            description: row.get(4).ok(),
            category: row.get(5).ok(),
            status: row.get(6).ok(),
        });
    }

    Ok(Json(competitions))
}

pub async fn create_competition(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
    Json(payload): Json<CreateCompetitionRequest>,
) -> Result<Json<Competition>, ApiError> {
    let id = Uuid::new_v4().to_string();
    let category = payload.category.clone().unwrap_or_else(|| "club_event".to_string());
    let status = payload.status.clone().unwrap_or_else(|| "scheduled".to_string());
    
    state.db.execute(
        "INSERT INTO competitions (id, title, date, location, description, category, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (id.clone(), payload.title.clone(), payload.date.clone(), payload.location.clone(), payload.description.clone(), category.clone(), status.clone()),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Competition {
        id,
        title: payload.title,
        date: payload.date,
        location: payload.location,
        description: payload.description,
        category: Some(category),
        status: Some(status),
    }))
}

pub async fn delete_competition(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
) -> Result<StatusCode, ApiError> {
    state.db.execute("DELETE FROM competitions WHERE id = ?1", [id])
        .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn update_competition(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
    Json(payload): Json<CreateCompetitionRequest>,
) -> Result<Json<Competition>, ApiError> {
    let category = payload.category.clone().unwrap_or_else(|| "club_event".to_string());
    let status = payload.status.clone().unwrap_or_else(|| "scheduled".to_string());
    
    state.db.execute(
        "UPDATE competitions SET title = ?1, date = ?2, location = ?3, description = ?4, category = ?5, status = ?6 WHERE id = ?7",
        (payload.title.clone(), payload.date.clone(), payload.location.clone(), payload.description.clone(), category.clone(), status.clone(), id.clone()),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Competition {
        id,
        title: payload.title,
        date: payload.date,
        location: payload.location,
        description: payload.description,
        category: Some(category),
        status: Some(status),
    }))
}
