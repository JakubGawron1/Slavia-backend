use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};

use crate::api_error::{api_error, ApiError};
use crate::db;
use crate::middleware::auth::{Claims, RequireAdminOrSuperAdmin, RequireSuperAdmin};
use crate::models::{Role, User};
use crate::notifications;
use crate::state::AppState;

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

async fn collect_users_for_sql(state: &AppState, sql: &str) -> Result<Vec<User>, ApiError> {
    let mut rows = state
        .db
        .query(sql, ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let role_str: String = row.get(4).unwrap();
        out.push(User {
            id: row.get(0).unwrap(),
            username: row.get(1).unwrap(),
            email: row.get(2).ok(),
            avatar_url: row.get(3).ok(),
            password_hash: "".to_string(),
            role: role_str.parse().unwrap(),
        });
    }
    Ok(out)
}

pub async fn list_admins(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<User>>, ApiError> {
    let sql = "SELECT id, username, email, avatar_url, role FROM users \
               WHERE role IN ('Admin', 'SuperAdmin', 'TrainerAdmin', 'Trainer', 'Athlete') \
               ORDER BY username ASC";
    let admins = collect_users_for_sql(&state, sql).await?;
    Ok(Json(admins))
}

#[derive(Serialize)]
pub struct GroupedAccounts {
    /// Admin, SuperAdmin, TrainerAdmin (admin-trener).
    pub staff_admins: Vec<User>,
    /// Trenerzy bez panelu admin oraz zawodnicy.
    pub club_members: Vec<User>,
}

pub async fn list_accounts_grouped(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<GroupedAccounts>, ApiError> {
    let staff_sql = "SELECT id, username, email, avatar_url, role FROM users \
                     WHERE role IN ('Admin', 'SuperAdmin', 'TrainerAdmin') \
                     ORDER BY role ASC, username ASC";
    let member_sql = "SELECT id, username, email, avatar_url, role FROM users \
                      WHERE role IN ('Trainer', 'Athlete') \
                      ORDER BY role ASC, username ASC";

    let staff_admins = collect_users_for_sql(&state, staff_sql).await?;
    let club_members = collect_users_for_sql(&state, member_sql).await?;

    Ok(Json(GroupedAccounts {
        staff_admins,
        club_members,
    }))
}

pub async fn create_admin(
    State(state): State<AppState>,
    auth: RequireSuperAdmin,
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

    let actor = notifications::username_by_id(state.db.as_ref(), &auth.0.sub)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "?".to_string());
    notifications::notify_admin_broadcast(
        &state,
        "admin_user_created",
        "Nowe konto administracyjne",
        &format!("{} utworzył konto „{}” (Admin).", actor, payload.username),
        Some(
            serde_json::json!({ "user_id": user_id.clone(), "username": payload.username.clone() }).to_string(),
        ),
    );

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

    let actor = notifications::username_by_id(state.db.as_ref(), &claims.sub)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "?".to_string());
    let target = notifications::username_by_id(state.db.as_ref(), &id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| id.clone());
    notifications::notify_admin_broadcast(
        &state,
        "admin_user_updated",
        "Zmiana konta użytkownika",
        &format!("{} zaktualizował konto użytkownika „{}”.", actor, target),
        Some(serde_json::json!({ "target_user_id": id }).to_string()),
    );

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

    let actor = notifications::username_by_id(state.db.as_ref(), &claims.sub)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "?".to_string());
    let target = notifications::username_by_id(state.db.as_ref(), &id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| id.clone());
    notifications::notify_admin_broadcast(
        &state,
        "admin_role_changed",
        "Zmiana roli",
        &format!(
            "{} ustawił rolę użytkownika „{}” na {}.",
            actor,
            target,
            payload.role
        ),
        Some(
            serde_json::json!({ "target_user_id": id, "role": payload.role.clone() }).to_string(),
        ),
    );

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
        let deleted_username = notifications::username_by_id(state.db.as_ref(), &id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| id.clone());
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
            .execute("DELETE FROM users WHERE id = ?1", [id.clone()])
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let actor = notifications::username_by_id(state.db.as_ref(), &claims.sub)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "?".to_string());
        notifications::notify_admin_broadcast(
            &state,
            "admin_user_deleted",
            "Usunięto konto",
            &format!("{} usunął konto użytkownika „{}”.", actor, deleted_username),
            None,
        );

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
    let mut prev_avatar: Option<String> = None;
    if payload.avatar_url.is_some() {
        let mut rows = state
            .db
            .query(
                "SELECT avatar_url FROM users WHERE id = ?1",
                [claims.sub.clone()],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            prev_avatar = row.get(0).ok();
        }
    }

    if let Some(ref new_av) = payload.avatar_url {
        if prev_avatar.as_ref() != Some(new_av) {
            if let Some(ref old) = prev_avatar {
                crate::cloudinary::destroy_if_cloudinary(&state, old).await;
            }
        }
    }

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
