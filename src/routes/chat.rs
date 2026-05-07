use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use crate::audit::write_audit_log;
use crate::middleware::auth::Claims;
use crate::models::Role;
use crate::notifications;
use crate::state::AppState;

#[derive(Serialize)]
pub struct ChatThreadDto {
    pub id: String,
    pub athlete_user_id: String,
    pub trainer_user_id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct ChatMessageDto {
    pub id: String,
    pub thread_id: String,
    pub sender_user_id: String,
    pub body: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_username: Option<String>,
    /// Cloudinary / URL: `users.avatar_url` lub — gdy puste — `athletes.image_url` nadawcy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_photo_url: Option<String>,
}

#[derive(Deserialize)]
pub struct OpenThreadRequest {
    pub athlete_user_id: String,
    pub trainer_user_id: String,
    pub title: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateThreadRequest {
    pub title: Option<String>,
}

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub body: String,
}

fn trim_opt_url(s: Option<String>) -> Option<String> {
    s.and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

async fn load_sender_display(
    state: &AppState,
    user_id: &str,
) -> Result<(Option<String>, Option<String>), ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT u.username,
                    CASE WHEN u.avatar_url IS NOT NULL AND trim(u.avatar_url) != '' THEN trim(u.avatar_url)
                         ELSE (SELECT image_url FROM athletes WHERE user_id = u.id ORDER BY id ASC LIMIT 1)
                    END AS sender_photo
             FROM users u WHERE u.id = ?1",
            [user_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(row) = row else {
        return Ok((None, None));
    };
    let username: Option<String> = row.get(0).ok();
    let photo_raw: Option<String> = row.get(1).ok();
    Ok((username, trim_opt_url(photo_raw)))
}

fn can_chat(claims: &Claims) -> bool {
    claims
        .roles
        .iter()
        .any(|r| matches!(r, Role::Athlete | Role::Trainer | Role::Admin | Role::SuperAdmin))
}

pub async fn open_thread(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<OpenThreadRequest>,
) -> Result<Json<ChatThreadDto>, ApiError> {
    if !can_chat(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień do czatu"));
    }
    let now = Utc::now().to_rfc3339();

    let mut rows = state
        .db
        .query(
            "SELECT id, title, created_at, updated_at FROM chat_threads WHERE athlete_user_id = ?1 AND trainer_user_id = ?2",
            (payload.athlete_user_id.clone(), payload.trainer_user_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Ok(Json(ChatThreadDto {
            id: row.get(0).unwrap_or_default(),
            athlete_user_id: payload.athlete_user_id,
            trainer_user_id: payload.trainer_user_id,
            title: row.get(1).ok(),
            created_at: row.get(2).unwrap_or_default(),
            updated_at: row.get(3).unwrap_or_default(),
        }));
    }

    let id = Uuid::new_v4().to_string();
    state
        .db
        .execute(
            "INSERT INTO chat_threads (id, athlete_user_id, trainer_user_id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id.clone(),
                payload.athlete_user_id.clone(),
                payload.trainer_user_id.clone(),
                payload.title.clone(),
                now.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ChatThreadDto {
        id,
        athlete_user_id: payload.athlete_user_id,
        trainer_user_id: payload.trainer_user_id,
        title: payload.title,
        created_at: now.clone(),
        updated_at: now,
    }))
}

pub async fn list_my_threads(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Vec<ChatThreadDto>>, ApiError> {
    if !can_chat(&claims) {
        return Ok(Json(vec![]));
    }
    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_user_id, trainer_user_id, title, created_at, updated_at
             FROM chat_threads
             WHERE athlete_user_id = ?1 OR trainer_user_id = ?1
             ORDER BY updated_at DESC",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(ChatThreadDto {
            id: row.get(0).unwrap_or_default(),
            athlete_user_id: row.get(1).unwrap_or_default(),
            trainer_user_id: row.get(2).unwrap_or_default(),
            title: row.get(3).ok(),
            created_at: row.get(4).unwrap_or_default(),
            updated_at: row.get(5).unwrap_or_default(),
        });
    }
    Ok(Json(out))
}

pub async fn update_thread(
    State(state): State<AppState>,
    claims: Claims,
    Path(thread_id): Path<String>,
    Json(payload): Json<UpdateThreadRequest>,
) -> Result<Json<ChatThreadDto>, ApiError> {
    let mut membership = state
        .db
        .query(
            "SELECT id, athlete_user_id, trainer_user_id, created_at FROM chat_threads WHERE id = ?1 AND (athlete_user_id = ?2 OR trainer_user_id = ?2)",
            (thread_id.clone(), claims.sub.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = membership
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::FORBIDDEN, "Brak dostępu do wątku"))?;

    let now = Utc::now().to_rfc3339();
    state
        .db
        .execute(
            "UPDATE chat_threads SET title = ?1, updated_at = ?2 WHERE id = ?3",
            (payload.title.clone(), now.clone(), thread_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ChatThreadDto {
        id: thread_id,
        athlete_user_id: row.get(1).unwrap_or_default(),
        trainer_user_id: row.get(2).unwrap_or_default(),
        title: payload.title,
        created_at: row.get(3).unwrap_or_default(),
        updated_at: now,
    }))
}

pub async fn list_messages(
    State(state): State<AppState>,
    claims: Claims,
    Path(thread_id): Path<String>,
) -> Result<Json<Vec<ChatMessageDto>>, ApiError> {
    let mut membership = state
        .db
        .query(
            "SELECT id FROM chat_threads WHERE id = ?1 AND (athlete_user_id = ?2 OR trainer_user_id = ?2)",
            (thread_id.clone(), claims.sub.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if membership
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak dostępu do wątku"));
    }

    let mut rows = state
        .db
        .query(
            "SELECT m.id, m.thread_id, m.sender_user_id, m.body, m.created_at, u.username AS sender_username,
                    CASE WHEN u.avatar_url IS NOT NULL AND trim(u.avatar_url) != '' THEN trim(u.avatar_url)
                         ELSE (SELECT image_url FROM athletes WHERE user_id = u.id ORDER BY id ASC LIMIT 1)
                    END AS sender_photo
             FROM chat_messages m
             JOIN users u ON u.id = m.sender_user_id
             WHERE m.thread_id = ?1
             ORDER BY m.created_at ASC",
            [thread_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let photo_raw: Option<String> = row.get(6).ok();
        out.push(ChatMessageDto {
            id: row.get(0).unwrap_or_default(),
            thread_id: row.get(1).unwrap_or_default(),
            sender_user_id: row.get(2).unwrap_or_default(),
            body: row.get(3).unwrap_or_default(),
            created_at: row.get(4).unwrap_or_default(),
            sender_username: row.get(5).ok(),
            sender_photo_url: trim_opt_url(photo_raw),
        });
    }

    state
        .db
        .execute(
            "INSERT INTO chat_reads (thread_id, user_id, last_read_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(thread_id, user_id) DO UPDATE SET last_read_at = excluded.last_read_at",
            (thread_id, claims.sub, Utc::now().to_rfc3339()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(out))
}

pub async fn send_message(
    State(state): State<AppState>,
    claims: Claims,
    Path(thread_id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<Json<ChatMessageDto>, ApiError> {
    let body = payload.body.trim();
    if body.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Treść wiadomości nie może być pusta"));
    }
    let mut membership = state
        .db
        .query(
            "SELECT athlete_user_id, trainer_user_id FROM chat_threads WHERE id = ?1 AND (athlete_user_id = ?2 OR trainer_user_id = ?2)",
            (thread_id.clone(), claims.sub.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = membership
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::FORBIDDEN, "Brak dostępu do wątku"))?;
    let athlete_uid: String = row.get(0).unwrap_or_default();
    let trainer_uid: String = row.get(1).unwrap_or_default();

    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    state
        .db
        .execute(
            "INSERT INTO chat_messages (id, thread_id, sender_user_id, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            (id.clone(), thread_id.clone(), claims.sub.clone(), body.to_string(), now.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state
        .db
        .execute(
            "UPDATE chat_threads SET updated_at = ?1 WHERE id = ?2",
            (now.clone(), thread_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let target_user = if claims.sub == athlete_uid {
        trainer_uid
    } else {
        athlete_uid
    };
    notifications::notify_admin_broadcast(
        &state,
        "chat_message",
        "Nowa wiadomość na czacie",
        "Wątek trener–zawodnik ma nową wiadomość.",
        Some(serde_json::json!({ "thread_id": thread_id, "target_user_id": target_user }).to_string()),
    );
    let _ = write_audit_log(
        state.db.as_ref(),
        Some(&claims.sub),
        Some("chat"),
        "chat",
        "message_sent",
        Some("thread"),
        Some(&thread_id),
        Some(&serde_json::json!({ "len": body.chars().count() }).to_string()),
    )
    .await;

    let (sender_username, sender_photo_url) = load_sender_display(&state, &claims.sub).await?;

    Ok(Json(ChatMessageDto {
        id,
        thread_id,
        sender_user_id: claims.sub,
        body: body.to_string(),
        created_at: now,
        sender_username,
        sender_photo_url,
    }))
}

pub async fn delete_thread(
    State(state): State<AppState>,
    claims: Claims,
    Path(thread_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !can_chat(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień do czatu"));
    }

    // Hard-delete: FK z ON DELETE CASCADE usuwa chat_messages i chat_reads.
    let n = state
        .db
        .execute(
            "DELETE FROM chat_threads WHERE id = ?1 AND (athlete_user_id = ?2 OR trainer_user_id = ?2)",
            (thread_id.clone(), claims.sub.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if n == 0 {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak dostępu do wątku"));
    }

    let _ = write_audit_log(
        state.db.as_ref(),
        Some(&claims.sub),
        Some("chat"),
        "chat",
        "thread_deleted",
        Some("thread"),
        Some(&thread_id),
        None,
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}
