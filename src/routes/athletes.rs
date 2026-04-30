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

#[derive(Deserialize)]
pub struct CreateAthleteRequest {
    pub full_name: String,
    pub birth_year: Option<i64>,
    pub weight_category: Option<String>,
    pub best_snatch_kg: Option<f64>,
    pub best_clean_jerk_kg: Option<f64>,
    pub total_kg: Option<f64>,
    pub notes: Option<String>,
    pub username: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateAthleteRequest {
    pub full_name: Option<String>,
    pub birth_year: Option<i64>,
    pub weight_category: Option<String>,
    pub best_snatch_kg: Option<f64>,
    pub best_clean_jerk_kg: Option<f64>,
    pub total_kg: Option<f64>,
    pub notes: Option<String>,
    pub is_active: Option<bool>,
}

pub async fn list_athletes(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<Athlete>>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, user_id, full_name, birth_year, weight_category, best_snatch_kg, best_clean_jerk_kg, total_kg, notes, is_active FROM athletes", ())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athletes = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        athletes.push(Athlete {
            id: row.get(0).unwrap_or_default(),
            user_id: row.get(1).ok(),
            full_name: row.get(2).unwrap_or_default(),
            birth_year: row.get(3).ok(),
            weight_category: row.get(4).ok(),
            best_snatch_kg: row.get(5).ok(),
            best_clean_jerk_kg: row.get(6).ok(),
            total_kg: row.get(7).ok(),
            notes: row.get(8).ok(),
            is_active: row.get::<i64>(9).unwrap_or(1) != 0,
        });
    }

    Ok(Json(athletes))
}

pub async fn list_athletes_public(
    State(state): State<AppState>,
) -> Result<Json<Vec<Athlete>>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, user_id, full_name, birth_year, weight_category, best_snatch_kg, best_clean_jerk_kg, total_kg, is_active FROM athletes WHERE is_active = 1", ())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athletes = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        athletes.push(Athlete {
            id: row.get(0).unwrap_or_default(),
            user_id: row.get(1).ok(),
            full_name: row.get(2).unwrap_or_default(),
            birth_year: row.get(3).ok(),
            weight_category: row.get(4).ok(),
            best_snatch_kg: row.get(5).ok(),
            best_clean_jerk_kg: row.get(6).ok(),
            total_kg: row.get(7).ok(),
            notes: None,
            is_active: row.get::<i64>(8).unwrap_or(1) != 0,
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
    state.db.execute(
        "INSERT INTO athletes (id, user_id, full_name, birth_year, weight_category, best_snatch_kg, best_clean_jerk_kg, total_kg, notes) VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (athlete_id.clone(), payload.full_name.clone(), payload.birth_year, payload.weight_category.clone(), payload.best_snatch_kg, payload.best_clean_jerk_kg, payload.total_kg, payload.notes.clone()),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Athlete {
        id: athlete_id,
        user_id: None,
        full_name: payload.full_name,
        birth_year: payload.birth_year,
        weight_category: payload.weight_category,
        best_snatch_kg: payload.best_snatch_kg,
        best_clean_jerk_kg: payload.best_clean_jerk_kg,
        total_kg: payload.total_kg,
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
    state.db.execute(
        "UPDATE athletes SET 
            full_name = COALESCE(?1, full_name),
            birth_year = ?2,
            weight_category = ?3,
            best_snatch_kg = ?4,
            best_clean_jerk_kg = ?5,
            total_kg = ?6,
            notes = ?7,
            is_active = COALESCE(?8, is_active)
         WHERE id = ?9",
        (
            payload.full_name,
            payload.birth_year,
            payload.weight_category,
            payload.best_snatch_kg,
            payload.best_clean_jerk_kg,
            payload.total_kg,
            payload.notes,
            payload.is_active.map(|v| if v { 1 } else { 0 }),
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
        .query("SELECT id, user_id, full_name, birth_year, weight_category, best_snatch_kg, best_clean_jerk_kg, total_kg, notes, is_active FROM athletes WHERE user_id = ?1", [claims.sub])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = row.ok_or((StatusCode::NOT_FOUND, "Athlete profile not found for this user".to_string()))?;

    Ok(Json(Athlete {
        id: row.get(0).unwrap_or_default(),
        user_id: row.get(1).ok(),
        full_name: row.get(2).unwrap_or_default(),
        birth_year: row.get(3).ok(),
        weight_category: row.get(4).ok(),
        best_snatch_kg: row.get(5).ok(),
        best_clean_jerk_kg: row.get(6).ok(),
        total_kg: row.get(7).ok(),
        notes: row.get(8).ok(),
        is_active: row.get::<i64>(9).unwrap_or(1) != 0,
    }))
}
