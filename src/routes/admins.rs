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
use crate::middleware::auth::{
    forbid_mutating_superadmin_user_record, Claims, RequireAdminOrSuperAdmin, RequireSuperAdmin,
};
use crate::models::{Role, User};
use crate::notifications;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateAdminRequest {
    pub username: String,
    pub password: String,
    /// Opcjonalnie — domyślnie `["Admin"]`. Bez duplikatów, co najmniej jedna rola.
    #[serde(default)]
    pub roles: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct UpdateUserRoleRequest {
    pub roles: Vec<String>,
}

/// Parsuje nazwy ról, usuwa duplikaty (kolejność pierwszego wystąpienia), wymaga ≥1 roli.
fn parse_roles_list(raw: &[String]) -> Result<Vec<Role>, ApiError> {
    if raw.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "At least one role is required",
        ));
    }
    let mut out: Vec<Role> = Vec::new();
    for s in raw {
        let role: Role = s
            .parse()
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, format!("Invalid role: {}", s)))?;
        if !out.contains(&role) {
            out.push(role);
        }
    }
    if out.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "At least one role is required",
        ));
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub email: Option<String>,
    pub password: Option<String>,
    pub avatar_url: Option<String>,
    pub ui_theme_preset: Option<String>,
    pub ui_color_mode: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateUserAccountRequest {
    pub username: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

fn has_staff_admin_access(roles: &[Role]) -> bool {
    roles
        .iter()
        .any(|r| matches!(r, Role::SuperAdmin | Role::Admin))
}

/// Priorytet sortowania w grupie administratorów (niższy = wyżej na liście).
fn admin_panel_rank(roles: &[Role]) -> u8 {
    if roles.contains(&Role::SuperAdmin) {
        0
    } else if roles.contains(&Role::Admin) {
        1
    } else {
        255
    }
}

#[derive(Clone, Copy)]
enum AccountListKind {
    Admins,
    Trainers,
    Athletes,
}

/// Jedna z trzech list kont — **bez duplikatów**: pierwsza pasująca kategoria (np. SuperAdmin+Trener trafia tylko do `admins`).
/// Kombinacje ról są widoczne w polu `roles` konta.
fn classify_user_bucket(roles: &[Role]) -> Option<AccountListKind> {
    if has_staff_admin_access(roles) {
        return Some(AccountListKind::Admins);
    }
    if roles.contains(&Role::Trainer) {
        return Some(AccountListKind::Trainers);
    }
    if roles.contains(&Role::Athlete) {
        return Some(AccountListKind::Athletes);
    }
    None
}

pub(crate) async fn user_roles_by_id(state: &AppState, id: &str) -> Result<Option<Vec<Role>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT roles FROM users WHERE id = ?1", [id.to_string()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = row {
        let roles_json: String = row.get(0).unwrap();
        let roles: Vec<Role> = serde_json::from_str(&roles_json).map_err(|_| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Invalid roles JSON in database",
            )
        })?;
        Ok(Some(roles))
    } else {
        Ok(None)
    }
}

async fn count_superadmin_accounts(state: &AppState) -> Result<i64, ApiError> {
    let all = collect_users_for_sql(
        state,
        "SELECT id, username, email, avatar_url, roles FROM users ORDER BY username ASC",
    )
    .await?;
    Ok(all
        .into_iter()
        .filter(|u| u.roles.contains(&Role::SuperAdmin))
        .count() as i64)
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
        let roles_json: String = row.get(4).unwrap();
        let roles: Vec<Role> = serde_json::from_str(&roles_json).unwrap();
        out.push(User {
            id: row.get(0).unwrap(),
            username: row.get(1).unwrap(),
            email: row.get(2).ok(),
            avatar_url: row.get(3).ok(),
            password_hash: "".to_string(),
            roles,
        });
    }
    Ok(out)
}

pub async fn list_admins(
    State(state): State<AppState>,
    auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<User>>, ApiError> {
    let sql = "SELECT id, username, email, avatar_url, roles FROM users ORDER BY username ASC";
    let all_users = collect_users_for_sql(&state, sql).await?;
    let caller_super = auth.0.roles.contains(&Role::SuperAdmin);
    let admins = all_users
        .into_iter()
        .filter(|u| {
            u.roles
                .iter()
                .any(|r| matches!(r, Role::Admin | Role::SuperAdmin | Role::Trainer | Role::Athlete))
        })
        .filter(|u| caller_super || !u.roles.contains(&Role::SuperAdmin))
        .collect();
    Ok(Json(admins))
}

#[derive(Serialize)]
pub struct GroupedAccounts {
    /// SuperAdmin, Admin — dostęp do panelu administracyjnego (trener bez Admina nie trafia tutaj).
    pub admins: Vec<User>,
    /// Trenerzy bez roli kadry administracyjnej (`Trainer`).
    pub trainers: Vec<User>,
    /// Zawodnicy z kontem (`Athlete`), bez roli admin ani trener.
    pub athletes: Vec<User>,
}

pub async fn list_accounts_grouped(
    State(state): State<AppState>,
    auth: RequireAdminOrSuperAdmin,
) -> Result<Json<GroupedAccounts>, ApiError> {
    let sql = "SELECT id, username, email, avatar_url, roles FROM users ORDER BY username ASC";
    let all_users = collect_users_for_sql(&state, sql).await?;
    let caller_super = auth.0.roles.contains(&Role::SuperAdmin);

    let mut admins = Vec::new();
    let mut trainers = Vec::new();
    let mut athletes = Vec::new();

    for user in all_users {
        if !caller_super && user.roles.contains(&Role::SuperAdmin) {
            continue;
        }
        match classify_user_bucket(&user.roles) {
            Some(AccountListKind::Admins) => admins.push(user),
            Some(AccountListKind::Trainers) => trainers.push(user),
            Some(AccountListKind::Athletes) => athletes.push(user),
            None => {}
        }
    }

    admins.sort_by(|a, b| {
        admin_panel_rank(&a.roles)
            .cmp(&admin_panel_rank(&b.roles))
            .then(a.username.cmp(&b.username))
    });
    trainers.sort_by(|a, b| a.username.cmp(&b.username));
    athletes.sort_by(|a, b| a.username.cmp(&b.username));

    Ok(Json(GroupedAccounts {
        admins,
        trainers,
        athletes,
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

    let roles_vec = match payload.roles.as_ref().filter(|r| !r.is_empty()) {
        Some(rs) => parse_roles_list(rs)?,
        None => vec![Role::Admin],
    };
    let roles_json =
        serde_json::to_string(&roles_vec).map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error serializing roles"))?;

    state.db.execute(
        "INSERT INTO users (id, username, password_hash, roles) VALUES (?1, ?2, ?3, ?4)",
        (user_id.clone(), payload.username.clone(), hash, roles_json),
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
        &format!(
            "{} utworzył konto „{}” z rolami {:?}.",
            actor, payload.username, roles_vec
        ),
        Some(
            serde_json::json!({ "user_id": user_id.clone(), "username": payload.username.clone(), "roles": roles_vec }).to_string(),
        ),
    );

    Ok(Json(User {
        id: user_id,
        username: payload.username,
        email: None,
        avatar_url: None,
        password_hash: "".to_string(),
        roles: roles_vec,
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

    let target_roles = user_roles_by_id(&state, &id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;

    forbid_mutating_superadmin_user_record(claims, &target_roles)?;

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
            "Use another SuperAdmin account to change your own roles",
        ));
    }

    let roles = parse_roles_list(&payload.roles)?;
    let roles_json = serde_json::to_string(&roles).map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error serializing roles"))?;

    let mut rows = state.db.query("SELECT roles FROM users WHERE id = ?1", [id.clone()])
        .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let target_roles_json: String = row.get(0).unwrap();
        let target_roles: Vec<Role> = serde_json::from_str(&target_roles_json).unwrap();
        forbid_mutating_superadmin_user_record(claims, &target_roles)?;
    }

    let result = state.db.execute(
        "UPDATE users SET roles = ?1 WHERE id = ?2",
        (roles_json, id.clone()),
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
        "Zmiana ról",
        &format!(
            "{} ustawił role użytkownika „{}” na {:?}.",
            actor,
            target,
            payload.roles
        ),
        Some(
            serde_json::json!({ "target_user_id": id, "roles": payload.roles.clone() }).to_string(),
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

    let target_roles = user_roles_by_id(&state, &id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;

    let deleted_username = notifications::username_by_id(state.db.as_ref(), &id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| id.clone());

    if target_roles.contains(&Role::SuperAdmin) {
        let n = count_superadmin_accounts(&state).await?;
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
                crate::cloudinary::destroy_if_cloudinary(&state, old, "image").await;
            }
        }
    }

    if let Some(ref new_password) = payload.password {
        let trimmed = new_password.trim();
        if !trimmed.is_empty() {
            let argon2 = Argon2::default();
            let salt = SaltString::generate(&mut OsRng);
            let hash = argon2.hash_password(trimmed.as_bytes(), &salt)
                .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Error hashing password"))?
                .to_string();

            state.db.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", (hash, claims.sub.clone()))
                .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
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

    if let Some(raw) = &payload.ui_theme_preset {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            state
                .db
                .execute(
                    "UPDATE users SET ui_theme_preset = NULL WHERE id = ?1",
                    [claims.sub.clone()],
                )
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        } else {
            const ALLOW_PRESET: &[&str] = &[
                "pink",
                "dark",
                "slavia",
                "iron",
                "arena",
                "platform",
                "midnight",
                "ruby",
                "neon",
                "blackgym",
            ];
            if !ALLOW_PRESET.contains(&trimmed) {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Invalid ui_theme_preset",
                ));
            }
            state
                .db
                .execute(
                    "UPDATE users SET ui_theme_preset = ?1 WHERE id = ?2",
                    (trimmed.to_string(), claims.sub.clone()),
                )
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
    }

    if let Some(raw) = &payload.ui_color_mode {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            state
                .db
                .execute(
                    "UPDATE users SET ui_color_mode = NULL WHERE id = ?1",
                    [claims.sub.clone()],
                )
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        } else {
            const ALLOW_MODE: &[&str] = &["light", "dark", "system"];
            if !ALLOW_MODE.contains(&trimmed) {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Invalid ui_color_mode",
                ));
            }
            state
                .db
                .execute(
                    "UPDATE users SET ui_color_mode = ?1 WHERE id = ?2",
                    (trimmed.to_string(), claims.sub.clone()),
                )
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
    }
    
    Ok(StatusCode::OK)
}
