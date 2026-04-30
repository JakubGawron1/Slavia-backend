use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::state::AppState;
use crate::models::Competition;
use crate::middleware::auth::RequireAdminOrSuperAdmin;

#[derive(Deserialize)]
pub struct CreateCompetitionRequest {
    pub title: String,
    pub date: String,
    pub location: String,
    pub description: Option<String>,
}

pub async fn list_competitions(
    State(state): State<AppState>,
) -> Result<Json<Vec<Competition>>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, title, date, location, description FROM competitions", ())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut competitions = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        // Handle Option correctly from query
        let description_val = row.get::<String>(4).ok();
        
        competitions.push(Competition {
            id: row.get(0).unwrap(),
            title: row.get(1).unwrap(),
            date: row.get(2).unwrap(),
            location: row.get(3).unwrap(),
            description: description_val,
        });
    }

    Ok(Json(competitions))
}

pub async fn create_competition(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<CreateCompetitionRequest>,
) -> Result<Json<Competition>, (StatusCode, String)> {
    let id = Uuid::new_v4().to_string();
    
    state.db.execute(
        "INSERT INTO competitions (id, title, date, location, description) VALUES (?1, ?2, ?3, ?4, ?5)",
        (id.clone(), payload.title.clone(), payload.date.clone(), payload.location.clone(), payload.description.clone()),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Competition {
        id,
        title: payload.title,
        date: payload.date,
        location: payload.location,
        description: payload.description,
    }))
}

pub async fn delete_competition(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, (StatusCode, String)> {
    state.db.execute("DELETE FROM competitions WHERE id = ?1", [id])
        .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn update_competition(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<CreateCompetitionRequest>,
) -> Result<Json<Competition>, (StatusCode, String)> {
    state.db.execute(
        "UPDATE competitions SET title = ?1, date = ?2, location = ?3, description = ?4 WHERE id = ?5",
        (payload.title.clone(), payload.date.clone(), payload.location.clone(), payload.description.clone(), id.clone()),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Competition {
        id,
        title: payload.title,
        date: payload.date,
        location: payload.location,
        description: payload.description,
    }))
}
