use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};

use crate::state::AppState;
use crate::models::Athlete;
use crate::middleware::auth::RequireAdminOrSuperAdmin;

#[derive(Deserialize)]
pub struct CreateAthleteRequest {
    pub username: String,
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    pub birth_year: Option<i64>,
    pub weight_category: Option<String>,
    pub notes: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateAthleteRequest {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub birth_year: Option<i64>,
    pub weight_category: Option<String>,
    pub notes: Option<String>,
    pub is_active: Option<bool>,
}

pub async fn list_athletes(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<Athlete>>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, user_id, first_name, last_name, birth_year, weight_category, notes, is_active FROM athletes", ())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athletes = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        athletes.push(Athlete {
            id: row.get(0).unwrap(),
            user_id: row.get(1).unwrap(),
            first_name: row.get(2).unwrap(),
            last_name: row.get(3).unwrap(),
            birth_year: row.get(4).unwrap(),
            weight_category: row.get(5).unwrap(),
            notes: row.get(6).unwrap(),
            is_active: row.get::<i64>(7).unwrap() != 0,
        });
    }

    Ok(Json(athletes))
}

pub async fn create_athlete(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<CreateAthleteRequest>,
) -> Result<Json<Athlete>, (StatusCode, String)> {
    let argon2 = Argon2::default();
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2.hash_password(payload.password.as_bytes(), &salt)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Error hashing password".to_string()))?
        .to_string();

    let user_id = Uuid::new_v4().to_string();
    
    state.db.execute(
        "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
        (user_id.clone(), payload.username, hash, "Athlete"),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let athlete_id = Uuid::new_v4().to_string();
    state.db.execute(
        "INSERT INTO athletes (id, user_id, first_name, last_name, birth_year, weight_category, notes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (athlete_id.clone(), user_id.clone(), payload.first_name.clone(), payload.last_name.clone(), payload.birth_year, payload.weight_category.clone(), payload.notes.clone()),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Athlete {
        id: athlete_id,
        user_id,
        first_name: payload.first_name,
        last_name: payload.last_name,
        birth_year: payload.birth_year,
        weight_category: payload.weight_category,
        notes: payload.notes,
        is_active: true,
    }))
}

pub async fn update_athlete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<UpdateAthleteRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(val) = payload.first_name {
        state.db.execute("UPDATE athletes SET first_name = ?1 WHERE id = ?2", (val, id.clone()))
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(val) = payload.last_name {
        state.db.execute("UPDATE athletes SET last_name = ?1 WHERE id = ?2", (val, id.clone()))
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(val) = payload.birth_year {
        state.db.execute("UPDATE athletes SET birth_year = ?1 WHERE id = ?2", (val, id.clone()))
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(val) = payload.weight_category {
        state.db.execute("UPDATE athletes SET weight_category = ?1 WHERE id = ?2", (val, id.clone()))
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(val) = payload.notes {
        state.db.execute("UPDATE athletes SET notes = ?1 WHERE id = ?2", (val, id.clone()))
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(val) = payload.is_active {
        state.db.execute("UPDATE athletes SET is_active = ?1 WHERE id = ?2", (if val { 1 } else { 0 }, id.clone()))
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok(StatusCode::OK)
}

pub async fn delete_athlete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, (StatusCode, String)> {
    let mut rows = state.db.query("SELECT user_id FROM athletes WHERE id = ?1", [id.clone()])
        .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    
    if let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let user_id: String = row.get(0).unwrap();
        state.db.execute("DELETE FROM athletes WHERE id = ?1", [id])
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state.db.execute("DELETE FROM users WHERE id = ?1", [user_id])
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, "Athlete not found".to_string()))
    }
}
