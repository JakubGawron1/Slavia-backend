use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;
use crate::state::AppState;
use crate::models::Athlete;
use crate::middleware::auth::{RequireAdminOrSuperAdmin, Claims};
use argon2::PasswordHasher;

#[derive(Deserialize)]
pub struct CreateAthleteRequest {
    pub full_name: String,
    pub birth_year: Option<i64>,
    pub gender: Option<String>,
    pub weight_category: Option<String>,
    pub bodyweight: Option<f64>,
    pub best_snatch_kg: Option<f64>,
    pub best_clean_jerk_kg: Option<f64>,
    pub total_kg: Option<f64>,
    pub image_url: Option<String>,
    pub notes: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateAthleteRequest {
    pub full_name: Option<String>,
    pub birth_year: Option<i64>,
    pub gender: Option<String>,
    pub weight_category: Option<String>,
    pub bodyweight: Option<f64>,
    pub best_snatch_kg: Option<f64>,
    pub best_clean_jerk_kg: Option<f64>,
    pub total_kg: Option<f64>,
    pub image_url: Option<String>,
    pub notes: Option<String>,
    pub is_active: Option<bool>,
    pub username: Option<String>,
    pub password: Option<String>,
}

pub async fn list_athletes(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<Athlete>>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, is_active FROM athletes", ())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athletes = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        athletes.push(Athlete {
            id: row.get(0).unwrap_or_default(),
            user_id: row.get(1).ok(),
            full_name: row.get(2).unwrap_or_default(),
            birth_year: row.get(3).ok(),
            gender: row.get(4).ok(),
            weight_category: row.get(5).ok(),
            bodyweight: row.get(6).ok(),
            best_snatch_kg: row.get(7).ok(),
            best_clean_jerk_kg: row.get(8).ok(),
            total_kg: row.get(9).ok(),
            image_url: row.get(10).ok(),
            notes: row.get(11).ok(),
            is_active: row.get::<i64>(12).unwrap_or(1) != 0,
        });
    }

    Ok(Json(athletes))
}

pub async fn list_athletes_public(
    State(state): State<AppState>,
) -> Result<Json<Vec<Athlete>>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, is_active FROM athletes WHERE is_active = 1", ())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athletes = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        athletes.push(Athlete {
            id: row.get(0).unwrap_or_default(),
            user_id: row.get(1).ok(),
            full_name: row.get(2).unwrap_or_default(),
            birth_year: row.get(3).ok(),
            gender: row.get(4).ok(),
            weight_category: row.get(5).ok(),
            bodyweight: row.get(6).ok(),
            best_snatch_kg: row.get(7).ok(),
            best_clean_jerk_kg: row.get(8).ok(),
            total_kg: row.get(9).ok(),
            image_url: row.get(10).ok(),
            notes: row.get(11).ok(),
            is_active: row.get::<i64>(12).unwrap_or(1) != 0,
        });
    }

    Ok(Json(athletes))
}

pub async fn create_athlete(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<CreateAthleteRequest>,
) -> Result<Json<Athlete>, (StatusCode, String)> {
    let athlete_id = Uuid::new_v4().to_string();
    let total = payload.best_snatch_kg.unwrap_or(0.0) + payload.best_clean_jerk_kg.unwrap_or(0.0);
    
    let mut user_id: Option<String> = None;

    // Optional user account creation
    if let Some(username) = payload.username {
        if !username.trim().is_empty() {
            let password = payload.password.unwrap_or_else(|| "Slavia2026".to_string());
            let argon2 = argon2::Argon2::default();
            let salt = argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
            let hash = argon2.hash_password(password.as_bytes(), &salt)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .to_string();
            
            let uid = Uuid::new_v4().to_string();
            state.db.execute(
                "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, 'Athlete')",
                (uid.clone(), username, hash),
            ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("User creation failed: {}", e)))?;
            
            user_id = Some(uid);
        }
    }

    state.db.execute(
        "INSERT INTO athletes (id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        (athlete_id.clone(), user_id.clone(), payload.full_name.clone(), payload.birth_year, payload.gender.clone(), payload.weight_category.clone(), payload.bodyweight, payload.best_snatch_kg, payload.best_clean_jerk_kg, total, payload.image_url.clone(), payload.notes.clone()),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Athlete {
        id: athlete_id,
        user_id,
        full_name: payload.full_name,
        birth_year: payload.birth_year,
        gender: payload.gender,
        weight_category: payload.weight_category,
        bodyweight: payload.bodyweight,
        best_snatch_kg: payload.best_snatch_kg,
        best_clean_jerk_kg: payload.best_clean_jerk_kg,
        total_kg: Some(total),
        image_url: payload.image_url,
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
    // 1. Get current athlete to check if user_id exists
    let mut rows = state.db.query("SELECT user_id FROM athletes WHERE id = ?1", [id.clone()])
        .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    
    let mut current_user_id: Option<String> = None;
    if let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        current_user_id = row.get(0).ok();
    }

    // 2. Handle optional account creation
    let mut user_id_to_set = current_user_id.clone();
    if current_user_id.is_none() {
        if let Some(username) = payload.username {
            if !username.trim().is_empty() {
                let password = payload.password.unwrap_or_else(|| "Slavia2026".to_string());
                let argon2 = argon2::Argon2::default();
                let salt = argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
                let hash = argon2.hash_password(password.as_bytes(), &salt)
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                    .to_string();
                
                let uid = Uuid::new_v4().to_string();
                state.db.execute(
                    "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, 'Athlete')",
                    (uid.clone(), username, hash),
                ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("User creation failed: {}", e)))?;
                
                user_id_to_set = Some(uid);
            }
        }
    }

    // 3. Calculate total
    let snatch = payload.best_snatch_kg.unwrap_or(0.0);
    let cj = payload.best_clean_jerk_kg.unwrap_or(0.0);
    let total = if payload.best_snatch_kg.is_some() || payload.best_clean_jerk_kg.is_some() {
        Some(snatch + cj)
    } else {
        payload.total_kg
    };

    state.db.execute(
        "UPDATE athletes SET 
            full_name = COALESCE(?1, full_name),
            birth_year = ?2,
            gender = ?3,
            weight_category = ?4,
            bodyweight = ?5,
            best_snatch_kg = ?6,
            best_clean_jerk_kg = ?7,
            total_kg = ?8,
            image_url = ?9,
            notes = ?10,
            is_active = COALESCE(?11, is_active),
            user_id = COALESCE(?12, user_id)
         WHERE id = ?13",
        (
            payload.full_name,
            payload.birth_year,
            payload.gender,
            payload.weight_category,
            payload.bodyweight,
            payload.best_snatch_kg,
            payload.best_clean_jerk_kg,
            total,
            payload.image_url,
            payload.notes,
            payload.is_active.map(|v| if v { 1 } else { 0 }),
            user_id_to_set,
            id
        ),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
        let user_id: Option<String> = row.get(0).ok();
        
        state.db.execute("DELETE FROM athletes WHERE id = ?1", [id])
            .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            
        if let Some(uid) = user_id {
            state.db.execute("DELETE FROM users WHERE id = ?1", [uid])
                .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, "Athlete not found".to_string()))
    }
}

pub async fn me_athlete_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Athlete>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, is_active FROM athletes WHERE user_id = ?1", [claims.sub])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = row.ok_or((StatusCode::NOT_FOUND, "Athlete profile not found for this user".to_string()))?;

    Ok(Json(Athlete {
        id: row.get(0).unwrap_or_default(),
        user_id: row.get(1).ok(),
        full_name: row.get(2).unwrap_or_default(),
        birth_year: row.get(3).ok(),
        gender: row.get(4).ok(),
        weight_category: row.get(5).ok(),
        bodyweight: row.get(6).ok(),
        best_snatch_kg: row.get(7).ok(),
        best_clean_jerk_kg: row.get(8).ok(),
        total_kg: row.get(9).ok(),
        image_url: row.get(10).ok(),
        notes: row.get(11).ok(),
        is_active: row.get::<i64>(12).unwrap_or(1) != 0,
    }))
}

#[derive(Deserialize)]
pub struct LinkUserRequest {
    pub username: String,
    pub password: Option<String>,
}

pub async fn link_athlete_to_user(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<LinkUserRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let argon2 = argon2::Argon2::default();
    let salt = argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let password = payload.password.unwrap_or_else(|| "Slavia2026".to_string());
    let hash = argon2.hash_password(password.as_bytes(), &salt)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .to_string();

    let user_id = Uuid::new_v4().to_string();
    
    // 1. Create user
    state.db.execute(
        "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, 'Athlete')",
        (user_id.clone(), payload.username, hash),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("User creation failed: {}", e)))?;
    
    // 2. Link to athlete
    state.db.execute(
        "UPDATE athletes SET user_id = ?1 WHERE id = ?2",
        (user_id, athlete_id),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}
