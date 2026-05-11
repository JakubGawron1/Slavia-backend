use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{Json, extract::State, http::StatusCode};
use chrono::{Duration, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::Claims;
use crate::models::Role;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// Gdy konto ma włączone TOTP — 6–8 cyfr z aplikacji authenticator.
    #[serde(default)]
    pub totp_code: Option<String>,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub roles: Vec<Role>,
    pub user_id: String,
}

pub async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let username_trim = payload.username.trim().to_string();
    if let Err(()) = crate::login_throttle::record_login_attempt(&username_trim) {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Zbyt wiele prób logowania. Spróbuj ponownie za kilka minut.",
        ));
    }

    let mut rows = state
        .db
        .query(
            "SELECT id, username, password_hash, roles, totp_secret, totp_enabled FROM users WHERE username = ?1",
            [username_trim.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = match row {
        Some(r) => r,
        None => {
            return Err(api_error(
                StatusCode::UNAUTHORIZED,
                "Invalid username or password",
            ));
        }
    };

    let user_id: String = row.get(0).map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("id error: {}", e),
        )
    })?;
    let _username: String = row.get(1).map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("username error: {}", e),
        )
    })?;
    let password_hash: String = row.get(2).map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("hash error: {}", e),
        )
    })?;
    let roles_json: String = row.get(3).map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("roles error: {}", e),
        )
    })?;
    let totp_secret: Option<String> = row.get(4).ok();
    let totp_enabled: i64 = row.get(5).unwrap_or(0);
    let roles: Vec<Role> = serde_json::from_str(&roles_json)
        .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Invalid roles in db"))?;

    let parsed_hash = PasswordHash::new(&password_hash)
        .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error parsing hash"))?;

    if Argon2::default()
        .verify_password(payload.password.as_bytes(), &parsed_hash)
        .is_err()
    {
        return Err(api_error(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password",
        ));
    }

    if totp_enabled != 0 {
        let sec = totp_secret
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .and_then(crate::routes::totp::decode_totp_secret_b32);
        let Some(raw) = sec else {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Konto ma włączone 2FA, ale brak sekretu — skontaktuj się z administratorem.",
            ));
        };
        let code = payload
            .totp_code
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(code) = code else {
            return Err(api_error(StatusCode::BAD_REQUEST, "totp_required"));
        };
        if !crate::routes::totp::totp_verify(&raw, code) {
            return Err(api_error(
                StatusCode::UNAUTHORIZED,
                "Invalid username or password",
            ));
        }
    }

    crate::login_throttle::clear_login_attempts(&username_trim);

    let exp = Utc::now()
        .checked_add_signed(Duration::days(1))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = crate::middleware::auth::Claims {
        sub: user_id.clone(),
        roles: roles.clone(),
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
        roles,
        user_id,
    }))
}

#[derive(Serialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub roles: Vec<Role>,
    /// Czy włączone jest drugie składnik logowania (TOTP).
    #[serde(default)]
    pub totp_enabled: bool,
    pub avatar_url: Option<String>,
    pub is_banned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banned_reason: Option<String>,
    /// Preset kolorystyczny (`slavia`, `iron`, …) — zapisany na koncie.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_theme_preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_color_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub athlete_gender: Option<String>,
    /// Zdjęcie z profilu sportowego (`athletes.image_url`), gdy konto jest powiązane ze zawodnikiem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub athlete_image_url: Option<String>,
    /// `athletes.id` pierwszego profilu powiązanego z kontem (`athletes.user_id`), gdy takie jest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub athlete_id: Option<String>,
}

pub async fn me_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<UserInfo>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT u.username, u.avatar_url, u.ui_theme_preset, u.ui_color_mode, a.gender, a.image_url, u.is_banned, u.banned_reason, u.totp_enabled, a.id AS athlete_prof_id
             FROM users u
             LEFT JOIN athletes a ON a.user_id = u.id
             WHERE u.id = ?1
             ORDER BY a.id ASC
             LIMIT 1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = row.ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "User not found"))?;

    let username: String = row
        .get(0)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let avatar_url: Option<String> = row.get(1).ok();
    let ui_theme_preset: Option<String> = row.get(2).ok();
    let ui_color_mode: Option<String> = row.get(3).ok();
    let athlete_gender: Option<String> = row.get(4).ok();
    let athlete_image_url: Option<String> = row.get(5).ok();
    let is_banned_i: i64 = row.get(6).unwrap_or(0);
    let banned_reason: Option<String> = row.get(7).ok();
    let totp_enabled_i: i64 = row.get(8).unwrap_or(0);
    let athlete_id_link: Option<String> = row.get(9).ok();

    Ok(Json(UserInfo {
        id: claims.sub,
        username,
        roles: claims.roles,
        totp_enabled: totp_enabled_i != 0,
        avatar_url,
        is_banned: is_banned_i != 0,
        banned_reason,
        ui_theme_preset,
        ui_color_mode,
        athlete_gender,
        athlete_image_url,
        athlete_id: athlete_id_link,
    }))
}
