use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{SecondsFormat, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::{claims_has_staff_access, claims_is_pure_athlete, Claims};
use crate::models::{Role, TrainingLogEntry};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateTrainingLogRequest {
    pub session_date: String,
    pub title: Option<String>,
    pub notes: String,
}

#[derive(Deserialize)]
pub struct UpdateTrainingLogRequest {
    pub session_date: Option<String>,
    pub title: Option<String>,
    pub notes: Option<String>,
}

async fn ensure_athlete_exists(state: &AppState, athlete_id: &str) -> Result<(), ApiError> {
    let mut rows = state
        .db
        .query("SELECT id FROM athletes WHERE id = ?1", [athlete_id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"));
    }

    Ok(())
}

async fn ensure_self_training_log_access(
    state: &AppState,
    claims: &Claims,
    athlete_id: &str,
    forbidden_message: &'static str,
) -> Result<(), ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id FROM athletes WHERE id = ?1 AND user_id = ?2",
            (athlete_id.to_string(), claims.sub.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(StatusCode::FORBIDDEN, forbidden_message));
    }
    Ok(())
}

async fn ensure_can_read_training_log(
    state: &AppState,
    claims: &Claims,
    athlete_id: &str,
) -> Result<(), ApiError> {
    if claims_has_staff_access(claims) {
        return ensure_athlete_exists(state, athlete_id).await;
    }
    if claims.roles.contains(&Role::Athlete) {
        return ensure_self_training_log_access(
            state,
            claims,
            athlete_id,
            "You can only read your own training diary",
        )
        .await;
    }
    Err(api_error(
        StatusCode::FORBIDDEN,
        "Insufficient permissions to read training log",
    ))
}

/// Tworzenie / zmiana / usuwanie wpisów: kadra (dowolny zawodnik) albo zawodnik wyłącznie we własnym dzienniku.
async fn ensure_can_write_training_log(
    state: &AppState,
    claims: &Claims,
    athlete_id: &str,
) -> Result<(), ApiError> {
    if claims_has_staff_access(claims) {
        return ensure_athlete_exists(state, athlete_id).await;
    }
    if claims.roles.contains(&Role::Athlete) {
        return ensure_self_training_log_access(
            state,
            claims,
            athlete_id,
            "You can only edit your own training diary",
        )
        .await;
    }
    Err(api_error(
        StatusCode::FORBIDDEN,
        "Insufficient permissions to edit training log",
    ))
}

/// Zawodnik może zmieniać lub usuwać wyłącznie wpisy utworzone przez siebie (`author_user_id`).
fn ensure_athlete_only_own_entries(claims: &Claims, entry: &TrainingLogEntry) -> Result<(), ApiError> {
    if !claims_is_pure_athlete(claims) {
        return Ok(());
    }
    match entry.author_user_id.as_deref() {
        Some(uid) if uid == claims.sub.as_str() => Ok(()),
        _ => Err(api_error(
            StatusCode::FORBIDDEN,
            "Athletes may only edit or delete their own diary entries",
        )),
    }
}

fn row_to_entry(row: &libsql::Row) -> TrainingLogEntry {
    TrainingLogEntry {
        id: row.get(0).unwrap(),
        athlete_id: row.get(1).unwrap(),
        session_date: row.get(2).unwrap(),
        title: row.get(3).ok(),
        notes: row.get(4).unwrap(),
        created_at: row.get(5).unwrap(),
        author_user_id: row.get(6).ok(),
        author_username: row.get(7).ok(),
    }
}

pub async fn list_training_log(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    claims: Claims,
) -> Result<Json<Vec<TrainingLogEntry>>, ApiError> {
    ensure_can_read_training_log(&state, &claims, &athlete_id).await?;

    let mut rows = state
        .db
        .query(
            "SELECT t.id, t.athlete_id, t.session_date, t.title, t.notes, t.created_at, \
             t.author_user_id, u.username \
             FROM training_log_entries t \
             LEFT JOIN users u ON u.id = t.author_user_id \
             WHERE t.athlete_id = ?1 \
             ORDER BY t.session_date DESC, t.created_at DESC",
            [athlete_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut list = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        list.push(row_to_entry(&row));
    }

    Ok(Json(list))
}

pub async fn create_training_log(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    claims: Claims,
    Json(payload): Json<CreateTrainingLogRequest>,
) -> Result<Json<TrainingLogEntry>, ApiError> {
    ensure_can_write_training_log(&state, &claims, &athlete_id).await?;

    let notes = payload.notes.trim();
    if notes.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "notes cannot be empty",
        ));
    }

    let session_date = payload.session_date.trim();
    if session_date.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "session_date cannot be empty",
        ));
    }

    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let title_clean = payload
        .title
        .as_ref()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    state
        .db
        .execute(
            "INSERT INTO training_log_entries (id, athlete_id, session_date, title, notes, author_user_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                id.clone(),
                athlete_id.clone(),
                session_date.to_string(),
                title_clean.clone(),
                notes.to_string(),
                claims.sub.clone(),
                created_at.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let username = fetch_username(&state, &claims.sub).await?;

    let conn_arc = state.db.raw().await;
    let athlete_display = crate::notifications::athlete_full_name(conn_arc.as_ref(), &athlete_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "Zawodnik".to_string());
    let author_label = username.as_deref().unwrap_or("Użytkownik");

    if claims_is_pure_athlete(&claims) {
        crate::notifications::notify_training_log_athlete_note(
            &state,
            &athlete_id,
            &athlete_display,
            author_label,
            session_date,
            notes,
        );
    } else {
        crate::notifications::notify_training_log_trainer_note(
            &state,
            &athlete_id,
            Some(author_label),
            session_date,
            notes,
        );
    }

    Ok(Json(TrainingLogEntry {
        id,
        athlete_id,
        session_date: session_date.to_string(),
        title: title_clean,
        notes: notes.to_string(),
        created_at,
        author_user_id: Some(claims.sub.clone()),
        author_username: username,
    }))
}

async fn fetch_username(state: &AppState, user_id: &str) -> Result<Option<String>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT username FROM users WHERE id = ?1", [user_id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(row.map(|r| r.get::<String>(0).unwrap()))
}

async fn entry_for_athlete(
    state: &AppState,
    athlete_id: &str,
    entry_id: &str,
) -> Result<Option<TrainingLogEntry>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT t.id, t.athlete_id, t.session_date, t.title, t.notes, t.created_at, \
             t.author_user_id, u.username \
             FROM training_log_entries t \
             LEFT JOIN users u ON u.id = t.author_user_id \
             WHERE t.id = ?1 AND t.athlete_id = ?2",
            (entry_id.to_string(), athlete_id.to_string()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(row.map(|r| row_to_entry(&r)))
}

pub async fn update_training_log(
    State(state): State<AppState>,
    Path((athlete_id, entry_id)): Path<(String, String)>,
    claims: Claims,
    Json(payload): Json<UpdateTrainingLogRequest>,
) -> Result<Json<TrainingLogEntry>, ApiError> {
    ensure_can_write_training_log(&state, &claims, &athlete_id).await?;

    let existing = entry_for_athlete(&state, &athlete_id, &entry_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Entry not found"))?;

    ensure_athlete_only_own_entries(&claims, &existing)?;

    if payload.session_date.is_none() && payload.title.is_none() && payload.notes.is_none() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "At least one field is required",
        ));
    }

    let session_date = payload
        .session_date
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.session_date.clone());

    let title = match &payload.title {
        None => existing.title.clone(),
        Some(t) => {
            let x = t.trim();
            if x.is_empty() {
                None
            } else {
                Some(x.to_string())
            }
        }
    };

    let notes = payload
        .notes
        .as_ref()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or(existing.notes.clone());

    if notes.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "notes cannot be empty"));
    }

    state
        .db
        .execute(
            "UPDATE training_log_entries SET session_date = ?1, title = ?2, notes = ?3 WHERE id = ?4 AND athlete_id = ?5",
            (
                session_date.clone(),
                title.clone(),
                notes.clone(),
                entry_id.clone(),
                athlete_id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let updated = entry_for_athlete(&state, &athlete_id, &entry_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Entry not found"))?;
    Ok(Json(updated))
}

pub async fn delete_training_log(
    State(state): State<AppState>,
    Path((athlete_id, entry_id)): Path<(String, String)>,
    claims: Claims,
) -> Result<StatusCode, ApiError> {
    ensure_can_write_training_log(&state, &claims, &athlete_id).await?;

    let existing = entry_for_athlete(&state, &athlete_id, &entry_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Entry not found"))?;

    ensure_athlete_only_own_entries(&claims, &existing)?;

    let r = state
        .db
        .execute(
            "DELETE FROM training_log_entries WHERE id = ?1 AND athlete_id = ?2",
            (entry_id, athlete_id),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if r == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Entry not found"));
    }

    Ok(StatusCode::NO_CONTENT)
}
