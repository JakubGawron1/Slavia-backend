use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use libsql::Row;
use serde::Deserialize;
use uuid::Uuid;
use crate::api_error::{api_error, ApiError};
use crate::models::{Athlete, AthletePublic, Role};
use crate::notifications;
use crate::state::AppState;
use crate::middleware::auth::{
    forbid_mutating_superadmin_user_record, Claims, RequireAdminOrSuperAdmin, RequireTrainerOrHigher,
};
use crate::routes::admins::user_roles_by_id;
use crate::sql_row;
use argon2::PasswordHasher;

/// Utworzenie konta (`users`) dla zawodnika — wyłącznie dla Admin / SuperAdmin (wywołanie jest wcześniej chronione).
async fn insert_athlete_user_account(
    state: &AppState,
    username: String,
    password: String,
) -> Result<String, ApiError> {
    let argon2 = argon2::Argon2::default();
    let salt = argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .to_string();
    let uid = Uuid::new_v4().to_string();
    let roles_json = serde_json::to_string(&vec![Role::Athlete]).map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    state
        .db
        .execute(
            "INSERT INTO users (id, username, password_hash, roles) VALUES (?1, ?2, ?3, ?4)",
            (uid.clone(), username, hash, roles_json),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("User creation failed: {}", e)))?;
    Ok(uid)
}

fn claims_may_create_athlete_user_account(claims: &Claims) -> bool {
    claims
        .roles
        .iter()
        .any(|r| matches!(r, Role::Admin | Role::SuperAdmin))
}

/// Jeśli podano login: Admin/SuperAdmin tworzy konto; sam trener — wysyła prośbę do administratorów (bez tworzenia `users`).
async fn try_attach_athlete_login_or_request(
    state: &AppState,
    claims: &Claims,
    athlete_id: &str,
    athlete_display_name: &str,
    username_opt: &Option<String>,
    password_opt: &Option<String>,
) -> Result<Option<String>, ApiError> {
    let Some(u_raw) = username_opt
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let proposed = u_raw.to_string();
    if claims_may_create_athlete_user_account(claims) {
        let password = password_opt
            .clone()
            .unwrap_or_else(|| "Slavia2026".to_string());
        let uid = insert_athlete_user_account(state, proposed, password).await?;
        Ok(Some(uid))
    } else {
        let trainer_name = notifications::username_by_id(state.db.as_ref(), &claims.sub)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "?".to_string());
        let body = format!(
            "Trener „{}” poprosił o utworzenie konta logowania dla zawodnika „{}”. Proponowany login: „{}”.",
            trainer_name, athlete_display_name, proposed
        );
        let payload = serde_json::json!({
            "athlete_id": athlete_id,
            "requested_by_user_id": claims.sub,
            "proposed_username": proposed,
        })
        .to_string();
        notifications::notify_admin_broadcast(
            state,
            "athlete_account_requested",
            "Prośba o konto zawodnika",
            &body,
            Some(payload),
        );
        Ok(None)
    }
}

fn athlete_from_row(row: &Row) -> Result<Athlete, libsql::Error> {
    Ok(Athlete {
        id: sql_row::string(row, 0)?,
        user_id: sql_row::opt_string(row, 1)?,
        full_name: sql_row::string(row, 2)?,
        birth_year: sql_row::opt_i64(row, 3)?,
        gender: sql_row::opt_string(row, 4)?,
        weight_category: sql_row::opt_string(row, 5)?,
        bodyweight: sql_row::opt_f64(row, 6)?,
        best_snatch_kg: sql_row::opt_f64(row, 7)?,
        best_clean_jerk_kg: sql_row::opt_f64(row, 8)?,
        total_kg: sql_row::opt_f64(row, 9)?,
        image_url: sql_row::opt_string(row, 10)?,
        notes: sql_row::opt_string(row, 11)?,
        profile_tagline: sql_row::opt_string(row, 12)?,
        public_bio: sql_row::opt_string(row, 13)?,
        is_active: sql_row::bool_active(row, 14)?,
    })
}

fn athlete_public_from_row(row: &Row) -> Result<AthletePublic, libsql::Error> {
    let a = athlete_from_row(row)?;
    Ok(AthletePublic {
        id: a.id,
        full_name: a.full_name,
        birth_year: a.birth_year,
        gender: a.gender,
        weight_category: a.weight_category,
        bodyweight: a.bodyweight,
        best_snatch_kg: a.best_snatch_kg,
        best_clean_jerk_kg: a.best_clean_jerk_kg,
        total_kg: a.total_kg,
        image_url: a.image_url,
        profile_tagline: a.profile_tagline,
        public_bio: a.public_bio,
        is_active: a.is_active,
    })
}

const ATHLETE_ROW_SQL: &str =
    "SELECT id, user_id, full_name, birth_year, gender, weight_category, bodyweight, \
     best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, profile_tagline, public_bio, is_active \
     FROM athletes";

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
    pub profile_tagline: Option<String>,
    pub public_bio: Option<String>,
    #[serde(default)]
    pub is_active: Option<bool>,
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
    pub profile_tagline: Option<String>,
    pub public_bio: Option<String>,
    pub is_active: Option<bool>,
    pub username: Option<String>,
    pub password: Option<String>,
}

pub async fn get_athlete_public(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<AthletePublic>, ApiError> {
    let sql = format!(
        "{} WHERE id = ?1 AND (is_active IS NULL OR is_active = 1)",
        ATHLETE_ROW_SQL
    );
    let mut rows = state
        .db
        .query(&sql, [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete not found"))?;

    let public = athlete_public_from_row(&row)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(public))
}

pub async fn list_athletes(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<Athlete>>, ApiError> {
    let mut rows = state
        .db
        .query(ATHLETE_ROW_SQL, ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athletes = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let a = athlete_from_row(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        athletes.push(a);
    }

    Ok(Json(athletes))
}

pub async fn list_athletes_public(
    State(state): State<AppState>,
) -> Result<Json<Vec<Athlete>>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!(
                "{} WHERE is_active IS NULL OR is_active = 1",
                ATHLETE_ROW_SQL
            ),
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athletes = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let a = athlete_from_row(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        athletes.push(a);
    }

    Ok(Json(athletes))
}

pub async fn create_athlete(
    State(state): State<AppState>,
    auth: RequireTrainerOrHigher,
    Json(payload): Json<CreateAthleteRequest>,
) -> Result<Json<Athlete>, ApiError> {
    let athlete_id = Uuid::new_v4().to_string();
    let total = payload.best_snatch_kg.unwrap_or(0.0) + payload.best_clean_jerk_kg.unwrap_or(0.0);

    let user_id = try_attach_athlete_login_or_request(
        &state,
        &auth.0,
        &athlete_id,
        &payload.full_name,
        &payload.username,
        &payload.password,
    )
    .await?;

    let is_active = payload.is_active.unwrap_or(true);
    state.db.execute(
        "INSERT INTO athletes (id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, profile_tagline, public_bio, is_active) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        (
            athlete_id.clone(),
            user_id.clone(),
            payload.full_name.clone(),
            payload.birth_year,
            payload.gender.clone(),
            payload.weight_category.clone(),
            payload.bodyweight,
            payload.best_snatch_kg,
            payload.best_clean_jerk_kg,
            total,
            payload.image_url.clone(),
            payload.notes.clone(),
            payload.profile_tagline.clone(),
            payload.public_bio.clone(),
            if is_active { 1 } else { 0 },
        ),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_admin_broadcast(
        &state,
        "admin_athlete_created",
        "Nowy zawodnik",
        &format!("Dodano zawodnika: {}.", payload.full_name),
        Some(serde_json::json!({ "athlete_id": athlete_id.clone() }).to_string()),
    );

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
        profile_tagline: payload.profile_tagline,
        public_bio: payload.public_bio,
        is_active,
    }))
}

pub async fn update_athlete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth: RequireTrainerOrHigher,
    Json(payload): Json<UpdateAthleteRequest>,
) -> Result<StatusCode, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT user_id, full_name FROM athletes WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut current_user_id: Option<String> = None;
    let mut existing_full_name: Option<String> = None;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        current_user_id = row.get(0).ok();
        existing_full_name = row.get(1).ok();
    }

    let display_name = payload
        .full_name
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or(existing_full_name.clone())
        .unwrap_or_else(|| "Zawodnik".to_string());

    let mut user_id_to_set = current_user_id.clone();
    if current_user_id.is_none() {
        if let Some(uid) = try_attach_athlete_login_or_request(
            &state,
            &auth.0,
            &id,
            &display_name,
            &payload.username,
            &payload.password,
        )
        .await?
        {
            user_id_to_set = Some(uid);
        }
    }

    // Calculate total
    let snatch = payload.best_snatch_kg.unwrap_or(0.0);
    let cj = payload.best_clean_jerk_kg.unwrap_or(0.0);
    let total = if payload.best_snatch_kg.is_some() || payload.best_clean_jerk_kg.is_some() {
        Some(snatch + cj)
    } else {
        payload.total_kg
    };

    let mut prev_img_row = state
        .db
        .query("SELECT image_url FROM athletes WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let prev_img: Option<String> = if let Some(pr) = prev_img_row
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        pr.get(0).ok()
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"));
    };

    if payload.image_url.as_ref() != prev_img.as_ref() {
        if let Some(ref old) = prev_img {
            crate::cloudinary::destroy_if_cloudinary(&state, old).await;
        }
    }

    state
        .db
        .execute(
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
            profile_tagline = ?11,
            public_bio = ?12,
            is_active = COALESCE(?13, is_active),
            user_id = COALESCE(?14, user_id)
         WHERE id = ?15",
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
            payload.profile_tagline,
            payload.public_bio,
            payload.is_active.map(|v| if v { 1 } else { 0 }),
            user_id_to_set,
            id.clone()
        ),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_admin_broadcast(
        &state,
        "admin_athlete_updated",
        "Zaktualizowano zawodnika",
        &format!("Zmieniono dane zawodnika (profil ID: {}).", id),
        Some(serde_json::json!({ "athlete_id": id }).to_string()),
    );

    Ok(StatusCode::OK)
}

pub async fn delete_athlete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, ApiError> {
    let claims = &auth.0;
    let mut rows = state.db.query("SELECT user_id, full_name FROM athletes WHERE id = ?1", [id.clone()])
        .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    
    if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let user_id: Option<String> = row.get(0).ok();
        let full_name: String = row.get(1).unwrap_or_else(|_| "?".to_string());

        if let Some(ref uid) = user_id {
            if let Some(roles) = user_roles_by_id(&state, uid).await? {
                forbid_mutating_superadmin_user_record(claims, &roles)?;
            }
        }
        
        state.db.execute("DELETE FROM athletes WHERE id = ?1", [id.clone()])
            .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            
        if let Some(uid) = user_id {
            state.db.execute("DELETE FROM users WHERE id = ?1", [uid])
                .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }

        notifications::notify_admin_broadcast(
            &state,
            "admin_athlete_deleted",
            "Usunięto zawodnika",
            &format!("Usunięto zawodnika: {}.", full_name),
            Some(serde_json::json!({ "athlete_id": id }).to_string()),
        );
        
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"))
    }
}

pub async fn me_athlete_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Athlete>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("{} WHERE user_id = ?1", ATHLETE_ROW_SQL),
            [claims.sub],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = row.ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete profile not found for this user"))?;

    let athlete = athlete_from_row(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(athlete))
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
) -> Result<StatusCode, ApiError> {
    let password = payload.password.unwrap_or_else(|| "Slavia2026".to_string());
    let linked_username = payload.username.clone();
    let user_id = insert_athlete_user_account(&state, payload.username, password).await?;
    
    // 2. Link to athlete
    state.db.execute(
        "UPDATE athletes SET user_id = ?1 WHERE id = ?2",
        (user_id.clone(), athlete_id.clone()),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_admin_broadcast(
        &state,
        "admin_athlete_linked",
        "Powiązano zawodnika z kontem",
        &format!(
            "Utworzono konto „{}” i powiązano z profilem zawodnika ({}).",
            linked_username, athlete_id
        ),
        Some(
            serde_json::json!({ "athlete_id": athlete_id, "user_id": user_id }).to_string(),
        ),
    );

    Ok(StatusCode::OK)
}
