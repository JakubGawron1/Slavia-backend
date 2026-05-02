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
use crate::db;
use crate::models::{User, Role};
use crate::middleware::auth::{RequireSuperAdmin, RequireAdminOrSuperAdmin, Claims};

#[derive(Deserialize)]
pub struct CreateAdminRequest {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct UpdateUserRoleRequest {
    pub role: String,
}

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub email: Option<String>,
    pub password: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateUserAccountRequest {
    pub username: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

pub async fn list_admins(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<User>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, username, email, avatar_url, role FROM users WHERE role IN ('Admin', 'SuperAdmin', 'TrainerAdmin', 'Trainer', 'Athlete')", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut admins = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let role_str: String = row.get(4).unwrap();
        admins.push(User {
            id: row.get(0).unwrap(),
            username: row.get(1).unwrap(),
            email: row.get(2).ok(),
            avatar_url: row.get(3).ok(),
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
        avatar_url: None,
        password_hash: "".to_string(),
        role: Role::Admin,
    }))
}

pub async fn update_user_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<UpdateUserAccountRequest>,
) -> Result<StatusCode, ApiError> {
    if payload.username.is_none() && payload.email.is_none() && payload.password.is_none() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "At least one of username, email, password is required",
        ));
    }

    let claims = &auth.0;
    if claims.sub == id {
        // Własne konto — użyj /api/auth/profile
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Use /api/auth/profile to change your own account",
        ));
    }

    let mut rows = state
        .db
        .query("SELECT role FROM users WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let target_role: String = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        row.get(0).unwrap()
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "User not found"));
    };

    if target_role == "SuperAdmin" && claims.role != Role::SuperAdmin {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Only SuperAdmin can modify SuperAdmin accounts",
        ));
    }

    if let Some(new_username) = &payload.username {
        if new_username.is_empty() {
            return Err(api_error(StatusCode::BAD_REQUEST, "username cannot be empty"));
        }
        state
            .db
            .execute(
                "UPDATE users SET username = ?1 WHERE id = ?2",
                (new_username.clone(), id.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if let Some(new_email) = &payload.email {
        state
            .db
            .execute(
                "UPDATE users SET email = ?1 WHERE id = ?2",
                (new_email.clone(), id.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if let Some(new_password) = &payload.password {
        if new_password.is_empty() {
            return Err(api_error(StatusCode::BAD_REQUEST, "password cannot be empty"));
        }
        let argon2 = Argon2::default();
        let salt = SaltString::generate(&mut OsRng);
        let hash = argon2
            .hash_password(new_password.as_bytes(), &salt)
            .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error hashing password"))?
            .to_string();
        state
            .db
            .execute(
                "UPDATE users SET password_hash = ?1 WHERE id = ?2",
                (hash, id.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok(StatusCode::OK)
}

pub async fn update_user_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth: RequireSuperAdmin,
    Json(payload): Json<UpdateUserRoleRequest>,
) -> Result<StatusCode, ApiError> {
    let claims = &auth.0;
    if claims.sub == id {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Use another SuperAdmin account to change your own role",
        ));
    }

    let role: Role = payload.role.parse().map_err(|_| api_error(StatusCode::BAD_REQUEST, "Invalid role"))?;
    let role_value = role.to_string();

    let result = state.db.execute(
        "UPDATE users SET role = ?1 WHERE id = ?2",
        (role_value, id.clone()),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if result == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "User not found"));
    }

    Ok(StatusCode::OK)
}

pub async fn reset_database(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
) -> Result<StatusCode, ApiError> {
    db::reset_database(&state.db).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::OK)
}

pub async fn delete_admin(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth: RequireSuperAdmin,
) -> Result<StatusCode, ApiError> {
    let claims = &auth.0;
    if claims.sub == id {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Cannot delete your own account",
        ));
    }

    let mut rows = state
        .db
        .query("SELECT role FROM users WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let role: String = row.get(0).unwrap();
        if role == "SuperAdmin" {
            let mut crow = state
                .db
                .query(
                    "SELECT COUNT(*) FROM users WHERE role = 'SuperAdmin'",
                    (),
                )
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let cnt_row = crow
                .next()
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .ok_or_else(|| {
                    api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to count SuperAdmin accounts",
                    )
                })?;
            let n: i64 = cnt_row.get(0).unwrap();
            if n <= 1 {
                return Err(api_error(
                    StatusCode::FORBIDDEN,
                    "Cannot delete the last SuperAdmin account",
                ));
            }
        }

        state
            .db
            .execute(
                "UPDATE athletes SET user_id = NULL WHERE user_id = ?1",
                [id.clone()],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        state
            .db
            .execute("DELETE FROM posts WHERE author_id = ?1", [id.clone()])
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        state
            .db
            .execute("DELETE FROM users WHERE id = ?1", [id])
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "User not found"))
    }
}

pub async fn update_profile(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<UpdateProfileRequest>,
) -> Result<StatusCode, ApiError> {
    if let Some(new_password) = payload.password {
        let argon2 = Argon2::default();
        let salt = SaltString::generate(&mut OsRng);
        let hash = argon2.hash_password(new_password.as_bytes(), &salt)
            .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error hashing password"))?
            .to_string();
        
        state.db.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", (hash, claims.sub.clone()))
            .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    
    if let Some(new_email) = payload.email {
        state.db.execute("UPDATE users SET email = ?1 WHERE id = ?2", (new_email, claims.sub.clone()))
            .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if let Some(url) = &payload.avatar_url {
        state
            .db
            .execute(
                "UPDATE users SET avatar_url = ?1 WHERE id = ?2",
                (url.clone(), claims.sub.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    
    Ok(StatusCode::OK)
}
