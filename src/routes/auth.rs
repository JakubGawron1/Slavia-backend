use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use jsonwebtoken::{encode, EncodingKey, Header};
use chrono::{Utc, Duration};

use crate::api_error::{api_error, ApiError};
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
) -> Result<Json<LoginResponse>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, username, password_hash, role FROM users WHERE username = ?1", [payload.username])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = match row {
        Some(r) => r,
        None => return Err(api_error(StatusCode::UNAUTHORIZED, "Invalid username or password")),
    };

    let user_id: String = row.get(0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("id error: {}", e)))?;
    let _username: String = row.get(1).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("username error: {}", e)))?;
    let password_hash: String = row.get(2).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("hash error: {}", e)))?;
    let role_str: String = row.get(3).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("role error: {}", e)))?;
    let role: Role = role_str.parse().map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Invalid role in db"))?;

    let parsed_hash = PasswordHash::new(&password_hash)
        .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error parsing hash"))?;

    if Argon2::default().verify_password(payload.password.as_bytes(), &parsed_hash).is_err() {
        return Err(api_error(StatusCode::UNAUTHORIZED, "Invalid username or password"));
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
    .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error creating token"))?;

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
    pub email: Option<String>,
    pub role: Role,
    pub avatar_url: Option<String>,
    /// Preset kolorystyczny (`slavia`, `iron`, …) — zapisany na koncie.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_theme_preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_color_mode: Option<String>,
}

pub async fn me_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<UserInfo>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT username, email, avatar_url, ui_theme_preset, ui_color_mode FROM users WHERE id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = row.ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "User not found"))?;
    
    let username: String = row.get(0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let email: Option<String> = row.get(1).ok();
    let avatar_url: Option<String> = row.get(2).ok();
    let ui_theme_preset: Option<String> = row.get(3).ok();
    let ui_color_mode: Option<String> = row.get(4).ok();

    Ok(Json(UserInfo {
        id: claims.sub,
        username,
        email,
        role: claims.role,
        avatar_url,
        ui_theme_preset,
        ui_color_mode,
    }))
}
