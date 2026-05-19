use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::{
    Claims, RequireAdminOrSuperAdmin, RequireTrainerOrHigher, claims_has_staff_access,
    forbid_mutating_superadmin_user_record,
};
use crate::models::{Athlete, AthletePublic, Role};
use crate::notifications;
use crate::routes::admins::user_roles_by_id;
use crate::sql_row;
use crate::state::AppState;
use argon2::PasswordHasher;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::DateTime;
use libsql::Row;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Utworzenie konta (`users`) dla zawodnika — wyłącznie dla Admin / SuperAdmin (wywołanie jest wcześniej chronione).
async fn insert_athlete_user_account(
    state: &AppState,
    username: String,
    password: String,
) -> Result<String, ApiError> {
    let argon2 = argon2::Argon2::default();
    let salt =
        argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .to_string();
    let uid = Uuid::new_v4().to_string();
    let roles_json = serde_json::to_string(&vec![Role::Athlete])
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state
        .db
        .execute(
            "INSERT INTO users (id, username, password_hash, roles) VALUES (?1, ?2, ?3, ?4)",
            (uid.clone(), username, hash, roles_json),
        )
        .await
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("User creation failed: {}", e),
            )
        })?;
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
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Slavia2026".to_string());
        let uid = insert_athlete_user_account(state, proposed, password).await?;
        Ok(Some(uid))
    } else {
        let conn_arc = state.db.raw().await;
        let trainer_name = notifications::username_by_id(conn_arc.as_ref(), &claims.sub)
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
        has_standing_order: sql_row::opt_i64(row, 15)?.unwrap_or(0) != 0,
    })
}

const ATHLETE_ROW_SQL: &str = "SELECT id, user_id, full_name, birth_year, gender, weight_category, bodyweight, \
     best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, profile_tagline, public_bio, is_active, has_standing_order \
     FROM athletes";

/// Jak na liście zawodników / profilu publicznym: najpierw zdjęcie konta (`users.avatar_url`), potem karta sportowa.
/// Kolumna `has_standing_order` zostaje na pozycji 15 (po `is_active`), `avatar_url` na 16 — żeby
/// `athlete_from_row_merge_public_photo` mogło użyć indeksu o 1 dalej niż w wariancie bez JOIN.
const ATHLETE_ROW_JOIN_USER_AVATAR_SQL: &str = "SELECT a.id, a.user_id, a.full_name, a.birth_year, a.gender, a.weight_category, a.bodyweight, \
     a.best_snatch_kg, a.best_clean_jerk_kg, a.total_kg, a.image_url, a.notes, a.profile_tagline, a.public_bio, a.is_active, a.has_standing_order, \
     u.avatar_url \
     FROM athletes a \
     LEFT JOIN users u ON u.id = a.user_id";

fn merge_profile_image(
    athlete_image_url: Option<String>,
    user_avatar_url: Option<String>,
) -> Option<String> {
    let user_av = user_avatar_url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if user_av.is_some() {
        return user_av;
    }
    athlete_image_url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn athlete_from_row_merge_public_photo(row: &Row) -> Result<Athlete, libsql::Error> {
    let mut a = athlete_from_row(row)?;
    let linked_avatar = sql_row::opt_string(row, 16)?;
    a.image_url = merge_profile_image(a.image_url.clone(), linked_avatar);
    Ok(a)
}

fn athlete_public_from_merged_row(row: &Row) -> Result<AthletePublic, libsql::Error> {
    let a = athlete_from_row_merge_public_photo(row)?;
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
        "{} WHERE a.id = ?1 AND (a.is_active IS NULL OR a.is_active = 1)",
        ATHLETE_ROW_JOIN_USER_AVATAR_SQL
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

    let public = athlete_public_from_merged_row(&row)
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
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let a = athlete_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
                "{} WHERE (a.is_active IS NULL OR a.is_active = 1)",
                ATHLETE_ROW_JOIN_USER_AVATAR_SQL
            ),
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athletes = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let a = athlete_from_row_merge_public_photo(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
        has_standing_order: false,
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
    if current_user_id.is_none()
        && let Some(uid) = try_attach_athlete_login_or_request(
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

    if payload.image_url.as_ref() != prev_img.as_ref()
        && let Some(ref old) = prev_img {
            crate::cloudinary::destroy_if_cloudinary(&state, old, "image").await;
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
                id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_admin_broadcast(
        &state,
        "admin_athlete_updated",
        "Zaktualizowano zawodnika",
        &format!("Zmieniono dane zawodnika: {}.", display_name),
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
    let mut rows = state
        .db
        .query(
            "SELECT user_id, full_name FROM athletes WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let user_id: Option<String> = row.get(0).ok();
        let full_name: String = row.get(1).unwrap_or_else(|_| "?".to_string());

        if let Some(ref uid) = user_id
            && let Some(roles) = user_roles_by_id(&state, uid).await? {
                forbid_mutating_superadmin_user_record(claims, &roles)?;
            }

        state
            .db
            .execute("DELETE FROM athletes WHERE id = ?1", [id.clone()])
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if let Some(uid) = user_id {
            state
                .db
                .execute("DELETE FROM users WHERE id = ?1", [uid])
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
            &format!("{} WHERE a.user_id = ?1", ATHLETE_ROW_JOIN_USER_AVATAR_SQL),
            [claims.sub],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = row.ok_or_else(|| {
        api_error(
            StatusCode::NOT_FOUND,
            "Athlete profile not found for this user",
        )
    })?;

    let athlete = athlete_from_row_merge_public_photo(&row)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(athlete))
}

#[derive(Deserialize)]
pub struct LinkUserRequest {
    pub username: String,
    pub password: Option<String>,
}

#[derive(Deserialize)]
pub struct AttachExistingUserRequest {
    pub user_id: String,
}

#[derive(Serialize)]
pub struct AthleteTimelineItem {
    pub id: String,
    pub kind: String,
    pub at: String,
    pub title: String,
    pub detail: String,
}

fn parse_sort_ts(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

fn can_view_athlete(claims: &Claims) -> bool {
    claims.roles.iter().any(|r| {
        matches!(
            r,
            Role::Athlete | Role::Trainer | Role::Admin | Role::SuperAdmin
        )
    })
}

pub async fn athlete_timeline(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    claims: Claims,
) -> Result<Json<Vec<AthleteTimelineItem>>, ApiError> {
    if !can_view_athlete(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak dostępu"));
    }
    if !claims_has_staff_access(&claims) {
        let mut owned = state
            .db
            .query(
                "SELECT id FROM athletes WHERE id = ?1 AND user_id = ?2",
                (athlete_id.clone(), claims.sub.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if owned
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_none()
        {
            return Err(api_error(
                StatusCode::FORBIDDEN,
                "Brak dostępu do osi czasu tego zawodnika",
            ));
        }
    }

    let mut out: Vec<AthleteTimelineItem> = Vec::new();

    let mut results = state
        .db
        .query(
            "SELECT r.id, r.date, r.snatch, r.clean_and_jerk, r.total, r.status, c.title \
             FROM results r \
             LEFT JOIN competitions c ON c.id = r.competition_id \
             WHERE r.athlete_id = ?1 \
             ORDER BY r.date DESC LIMIT 50",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(r) = results
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id: String = r.get(0).unwrap_or_default();
        let at: String = r.get(1).unwrap_or_default();
        let snatch: f64 = r.get(2).unwrap_or(0.0);
        let cj: f64 = r.get(3).unwrap_or(0.0);
        let total: f64 = r.get(4).unwrap_or(0.0);
        let status: String = r.get(5).unwrap_or_default();
        let comp_title: Option<String> = r.get(6).ok();

        let display_title = if let Some(t) = comp_title {
            format!("{} ({} kg, {})", t, total, status)
        } else {
            format!("Wynik {} kg ({})", total, status)
        };

        out.push(AthleteTimelineItem {
            id,
            kind: "result".to_string(),
            at,
            title: display_title,
            detail: format!("Rwanie {} kg · Podrzut {} kg", snatch, cj),
        });
    }

    let mut attendance = state
        .db
        .query(
            "SELECT id, session_date, status, verification_state FROM attendance_records WHERE athlete_id = ?1 ORDER BY session_date DESC LIMIT 50",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(r) = attendance
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id: String = r.get(0).unwrap_or_default();
        let at: String = r.get(1).unwrap_or_default();
        let status: String = r.get(2).unwrap_or_default();
        let verification: String = r.get(3).unwrap_or_default();
        out.push(AthleteTimelineItem {
            id,
            kind: "attendance".to_string(),
            at,
            title: format!("Obecność: {}", status),
            detail: format!("Weryfikacja: {}", verification),
        });
    }

    let mut logs = state
        .db
        .query(
            "SELECT id, session_date, title, notes FROM training_log_entries WHERE athlete_id = ?1 ORDER BY session_date DESC, created_at DESC LIMIT 50",
            [athlete_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(r) = logs
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id: String = r.get(0).unwrap_or_default();
        let at: String = r.get(1).unwrap_or_default();
        let title: String = r.get(2).unwrap_or_else(|_| "Wpis treningowy".to_string());
        let notes: String = r.get(3).unwrap_or_default();
        out.push(AthleteTimelineItem {
            id,
            kind: "training_log".to_string(),
            at,
            title,
            detail: notes,
        });
    }

    out.sort_by_key(|b| std::cmp::Reverse(parse_sort_ts(&b.at)));
    Ok(Json(out))
}

pub async fn attach_existing_user_to_athlete(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<AttachExistingUserRequest>,
) -> Result<StatusCode, ApiError> {
    let user_id = payload.user_id.trim();
    if user_id.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "user_id is required"));
    }
    let user_id = user_id.to_string();

    let sql = format!("{} WHERE id = ?1", ATHLETE_ROW_SQL);
    let mut rows = state
        .db
        .query(&sql, [athlete_id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete not found"))?;
    let athlete = athlete_from_row(&row)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if athlete
        .user_id
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Athlete already linked to a user account",
        ));
    }

    let roles = user_roles_by_id(&state, &user_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;
    if !roles.contains(&Role::Athlete) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "User must have Athlete role to attach to an athlete profile",
        ));
    }

    state
        .db
        .execute(
            "UPDATE athletes SET user_id = NULL WHERE user_id = ?1 AND id != ?2",
            (user_id.clone(), athlete_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .db
        .execute(
            "UPDATE athletes SET user_id = ?1 WHERE id = ?2",
            (user_id.clone(), athlete_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let conn_arc = state.db.raw().await;
    let login = notifications::username_by_id(conn_arc.as_ref(), &user_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "(login niedostępny)".to_string());

    notifications::notify_admin_broadcast(
        &state,
        "admin_athlete_linked_existing",
        "Powiązano istniejące konto ze zawodnikiem",
        &format!(
            "Powiązano konto „{}” ze zawodnikiem {}.",
            login, athlete.full_name
        ),
        Some(serde_json::json!({ "athlete_id": athlete_id, "user_id": user_id }).to_string()),
    );

    Ok(StatusCode::NO_CONTENT)
}

pub async fn detach_user_from_athlete(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT full_name FROM athletes WHERE id = ?1",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let athlete_name = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        row.get::<String>(0)
            .unwrap_or_else(|_| "Zawodnik".to_string())
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"));
    };

    let n = state
        .db
        .execute(
            "UPDATE athletes SET user_id = NULL WHERE id = ?1",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"));
    }

    notifications::notify_admin_broadcast(
        &state,
        "admin_athlete_detached_user",
        "Odłączono konto od zawodnika",
        &format!("Odłączono konto logowania od zawodnika: {}.", athlete_name),
        Some(serde_json::json!({ "athlete_id": athlete_id }).to_string()),
    );

    Ok(StatusCode::NO_CONTENT)
}

pub async fn link_athlete_to_user(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<LinkUserRequest>,
) -> Result<StatusCode, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT full_name FROM athletes WHERE id = ?1",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let athlete_name = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        row.get::<String>(0)
            .unwrap_or_else(|_| "Zawodnik".to_string())
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"));
    };

    let password = payload.password.unwrap_or_else(|| "Slavia2026".to_string());
    let linked_username = payload.username.clone();
    let user_id = insert_athlete_user_account(&state, payload.username, password).await?;

    // 2. Link to athlete
    state
        .db
        .execute(
            "UPDATE athletes SET user_id = ?1 WHERE id = ?2",
            (user_id.clone(), athlete_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_admin_broadcast(
        &state,
        "admin_athlete_linked",
        "Powiązano zawodnika z kontem",
        &format!(
            "Utworzono konto „{}” i powiązano z zawodnikiem {}.",
            linked_username, athlete_name
        ),
        Some(serde_json::json!({ "athlete_id": athlete_id, "user_id": user_id }).to_string()),
    );

    Ok(StatusCode::OK)
}
pub async fn update_my_athlete_profile_handler(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<UpdateAthleteRequest>,
) -> Result<StatusCode, ApiError> {
    if !claims.roles.contains(&Role::Athlete) {
        return Err(api_error(StatusCode::FORBIDDEN, "Tylko zawodnicy mogą edytować swój profil sportowy tą drogą."));
    }

    let mut rows = state
        .db
        .query(
            "SELECT id, full_name FROM athletes WHERE user_id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = row.ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Nie znaleziono Twojej karty zawodnika."))?;
    
    let athlete_id: String = row.get(0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let existing_full_name: String = row.get(1).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Dla zawodnika ograniczamy co może zmienić — tylko dane biometryczne/profilowe, nie notes ani is_active.
    state.db.execute(
        "UPDATE athletes SET 
            birth_year = COALESCE(?1, birth_year),
            gender = COALESCE(?2, gender),
            weight_category = COALESCE(?3, weight_category),
            bodyweight = COALESCE(?4, bodyweight),
            profile_tagline = COALESCE(?5, profile_tagline),
            public_bio = COALESCE(?6, public_bio)
         WHERE id = ?7",
        (
            payload.birth_year,
            payload.gender,
            payload.weight_category,
            payload.bodyweight,
            payload.profile_tagline,
            payload.public_bio,
            athlete_id.clone(),
        )
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_admin_broadcast(
        &state,
        "athlete_self_updated",
        "Zawodnik zaktualizował profil",
        &format!("Zawodnik „{}” zaktualizował swoje dane profilowe.", existing_full_name),
        Some(serde_json::json!({ "athlete_id": athlete_id }).to_string()),
    );

    Ok(StatusCode::OK)
}
