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

use crate::api_error::{api_error, ApiError};
use crate::state::AppState;
use crate::models::{User, Role};
use crate::middleware::auth::RequireSuperAdmin;

#[derive(Deserialize)]
pub struct CreateAdminRequest {
    pub username: String,
    pub password: String,
}

pub async fn list_admins(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
) -> Result<Json<Vec<User>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, username, email, role FROM users WHERE role = 'Admin' OR role = 'SuperAdmin'", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut admins = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let role_str: String = row.get(3).unwrap();
        admins.push(User {
            id: row.get(0).unwrap(),
            username: row.get(1).unwrap(),
            email: row.get(2).ok(),
            password_hash: "".to_string(), // hidden
            role: role_str.parse().unwrap(),
        });
    }

    Ok(Json(admins))
}

pub async fn create_admin(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Json(payload): Json<CreateAdminRequest>,
) -> Result<Json<User>, ApiError> {
    let argon2 = Argon2::default();
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2.hash_password(payload.password.as_bytes(), &salt)
        .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error hashing password"))?
        .to_string();

    let user_id = Uuid::new_v4().to_string();
    
    state.db.execute(
        "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
        (user_id.clone(), payload.username.clone(), hash, "Admin"),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(User {
        id: user_id,
        username: payload.username,
        email: None,
        password_hash: "".to_string(),
        role: Role::Admin,
    }))
}

pub async fn delete_admin(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireSuperAdmin,
) -> Result<StatusCode, ApiError> {
    let mut rows = state.db.query("SELECT role FROM users WHERE id = ?1", [id.clone()])
        .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    
    if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let role: String = row.get(0).unwrap();
        if role == "SuperAdmin" {
            return Err(api_error(StatusCode::FORBIDDEN, "Cannot delete SuperAdmin"));
        }
        
        state.db.execute("DELETE FROM users WHERE id = ?1", [id])
            .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "Admin not found"))
    }
}
