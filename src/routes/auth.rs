use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use jsonwebtoken::{encode, EncodingKey, Header};
use chrono::{Utc, Duration};

use crate::state::AppState;
use crate::models::Role;
use crate::middleware::auth::Claims;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub role: Role,
    pub user_id: String,
}

pub async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, username, password_hash, role FROM users WHERE username = ?1", [payload.username])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = match row {
        Some(r) => r,
        None => return Err((StatusCode::UNAUTHORIZED, "Invalid username or password".to_string())),
    };

    let user_id: String = row.get(0).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("id error: {}", e)))?;
    let _username: String = row.get(1).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("username error: {}", e)))?;
    let password_hash: String = row.get(2).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("hash error: {}", e)))?;
    let role_str: String = row.get(3).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("role error: {}", e)))?;
    let role: Role = role_str.parse().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Invalid role in db".to_string()))?;

    let parsed_hash = PasswordHash::new(&password_hash)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Error parsing hash".to_string()))?;

    if Argon2::default().verify_password(payload.password.as_bytes(), &parsed_hash).is_err() {
        return Err((StatusCode::UNAUTHORIZED, "Invalid username or password".to_string()));
    }

    let exp = Utc::now()
        .checked_add_signed(Duration::days(1))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = crate::middleware::auth::Claims {
        sub: user_id.clone(),
        role: role.clone(),
        exp,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_ref()),
    )
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Error creating token".to_string()))?;

    Ok(Json(LoginResponse {
        token,
        role,
        user_id,
    }))
}

#[derive(Serialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub role: Role,
}

pub async fn me_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<UserInfo>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT username FROM users WHERE id = ?1", [claims.sub.clone()])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = row.ok_or((StatusCode::UNAUTHORIZED, "User not found".to_string()))?;
    
    let username: String = row.get(0).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(UserInfo {
        id: claims.sub,
        username,
        role: claims.role,
    }))
}
