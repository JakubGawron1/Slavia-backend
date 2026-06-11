use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};

use crate::pagination::{ListPaginationQuery, parse_list_pagination};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::chat_cleanup::{CHAT_INACTIVITY_DAYS, prune_inactive_chat_threads};
use crate::middleware::auth::{Claims, RequireAdminOrSuperAdmin};
use crate::models::Role;
use crate::routes::admins::user_roles_by_id;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_last_seen_at: Option<String>,
    #[serde(default)]
    pub peer_online: bool,
}

#[derive(Serialize, Clone)]
pub struct ChatReactionSummary {
    pub emoji: String,
    pub count: i64,
    pub reacted_by_me: bool,
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
    #[serde(default)]
    pub reactions: Vec<ChatReactionSummary>,
}

#[derive(Deserialize)]
pub struct ToggleReactionRequest {
    pub emoji: String,
}

const PRESENCE_ONLINE_SECS: i64 = 300;

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

async fn touch_user_presence(state: &AppState, user_id: &str) {
    let now = Utc::now().to_rfc3339();
    let _ = state
        .db
        .execute(
            "UPDATE users SET last_seen_at = ?1 WHERE id = ?2",
            (now, user_id.to_string()),
        )
        .await;
}

fn is_recent_presence(ts: Option<&str>) -> bool {
    let Some(raw) = ts else {
        return false;
    };
    let Ok(dt) = DateTime::parse_from_rfc3339(raw.trim()) else {
        return false;
    };
    let age = Utc::now().signed_duration_since(dt.with_timezone(&Utc));
    age.num_seconds() >= 0 && age.num_seconds() <= PRESENCE_ONLINE_SECS
}

async fn peer_presence_for_thread(
    state: &AppState,
    athlete_uid: &str,
    trainer_uid: &str,
    viewer_id: &str,
) -> Result<(Option<String>, bool), ApiError> {
    let peer_id = if viewer_id == athlete_uid {
        trainer_uid
    } else {
        athlete_uid
    };
    let mut rows = state
        .db
        .query(
            "SELECT last_seen_at FROM users WHERE id = ?1 LIMIT 1",
            [peer_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let last_seen: Option<String> = row.and_then(|r| r.get(0).ok());
    let online = is_recent_presence(last_seen.as_deref());
    Ok((last_seen, online))
}

async fn load_reactions_for_thread(
    state: &AppState,
    thread_id: &str,
    viewer_id: &str,
) -> Result<std::collections::HashMap<String, Vec<ChatReactionSummary>>, ApiError> {
    use std::collections::HashMap;
    let mut rows = state
        .db
        .query(
            "SELECT r.message_id, r.emoji, COUNT(*) AS cnt,
                    SUM(CASE WHEN r.user_id = ?1 THEN 1 ELSE 0 END) AS mine
             FROM chat_message_reactions r
             INNER JOIN chat_messages m ON m.id = r.message_id
             WHERE m.thread_id = ?2
             GROUP BY r.message_id, r.emoji",
            (viewer_id.to_string(), thread_id.to_string()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut map: HashMap<String, Vec<ChatReactionSummary>> = HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let mid: String = row.get(0).unwrap_or_default();
        let emoji: String = row.get(1).unwrap_or_default();
        let count: i64 = row.get(2).unwrap_or(0);
        let mine: i64 = row.get(3).unwrap_or(0);
        map.entry(mid).or_default().push(ChatReactionSummary {
            emoji,
            count,
            reacted_by_me: mine > 0,
        });
    }
    Ok(map)
}

#[allow(clippy::too_many_arguments)]
async fn build_thread_dto(
    state: &AppState,
    viewer_id: &str,
    id: String,
    athlete_user_id: String,
    trainer_user_id: String,
    title: Option<String>,
    created_at: String,
    updated_at: String,
) -> Result<ChatThreadDto, ApiError> {
    let (peer_last_seen_at, peer_online) =
        peer_presence_for_thread(state, &athlete_user_id, &trainer_user_id, viewer_id).await?;
    Ok(ChatThreadDto {
        id,
        athlete_user_id,
        trainer_user_id,
        title,
        created_at,
        updated_at,
        peer_last_seen_at,
        peer_online,
    })
}

fn can_chat(claims: &Claims) -> bool {
    claims.roles.iter().any(|r| {
        matches!(
            r,
            Role::Athlete | Role::Trainer | Role::Admin | Role::SuperAdmin
        )
    })
}

pub async fn open_thread(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<OpenThreadRequest>,
) -> Result<Json<ChatThreadDto>, ApiError> {
    if !can_chat(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień do czatu"));
    }

    let athlete_uid = payload.athlete_user_id.trim();
    let trainer_uid = payload.trainer_user_id.trim();
    if athlete_uid.is_empty() || trainer_uid.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Wymagane athlete_user_id i trainer_user_id",
        ));
    }
    if athlete_uid == trainer_uid {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Uczestnicy wątku muszą być różni",
        ));
    }
    if claims.sub != athlete_uid && claims.sub != trainer_uid {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Możesz otworzyć wątek tylko jako uczestnik",
        ));
    }

    let athlete_roles = user_roles_by_id(&state, athlete_uid)
        .await?
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Nie znaleziono użytkownika zawodnika"))?;
    let trainer_roles = user_roles_by_id(&state, trainer_uid)
        .await?
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Nie znaleziono użytkownika trenera"))?;

    if !athlete_roles.contains(&Role::Athlete) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "athlete_user_id musi mieć rolę Athlete",
        ));
    }
    if !trainer_roles.iter().any(|r| {
        matches!(r, Role::Trainer | Role::Admin | Role::SuperAdmin)
    }) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "trainer_user_id musi mieć rolę kadry",
        ));
    }

    let athlete_user_id = athlete_uid.to_string();
    let trainer_user_id = trainer_uid.to_string();
    let now = Utc::now().to_rfc3339();

    let mut rows = state
        .db
        .query(
            "SELECT id, title, created_at, updated_at FROM chat_threads WHERE athlete_user_id = ?1 AND trainer_user_id = ?2",
            (athlete_user_id.clone(), trainer_user_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let dto = build_thread_dto(
            &state,
            &claims.sub,
            row.get(0).unwrap_or_default(),
            athlete_user_id.clone(),
            trainer_user_id.clone(),
            row.get(1).ok(),
            row.get(2).unwrap_or_default(),
            row.get(3).unwrap_or_default(),
        )
        .await?;
        return Ok(Json(dto));
    }

    let id = Uuid::new_v4().to_string();
    state
        .db
        .execute(
            "INSERT INTO chat_threads (id, athlete_user_id, trainer_user_id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id.clone(),
                athlete_user_id.clone(),
                trainer_user_id.clone(),
                payload.title.clone(),
                now.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let dto = build_thread_dto(
        &state,
        &claims.sub,
        id,
        athlete_user_id,
        trainer_user_id,
        payload.title,
        now.clone(),
        now,
    )
    .await?;
    Ok(Json(dto))
}

pub async fn list_my_threads(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Vec<ChatThreadDto>>, ApiError> {
    if !can_chat(&claims) {
        return Ok(Json(vec![]));
    }
    touch_user_presence(&state, &claims.sub).await;
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
        let dto = build_thread_dto(
            &state,
            &claims.sub,
            row.get(0).unwrap_or_default(),
            row.get(1).unwrap_or_default(),
            row.get(2).unwrap_or_default(),
            row.get(3).ok(),
            row.get(4).unwrap_or_default(),
            row.get(5).unwrap_or_default(),
        )
        .await?;
        out.push(dto);
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

    let dto = build_thread_dto(
        &state,
        &claims.sub,
        thread_id,
        row.get(1).unwrap_or_default(),
        row.get(2).unwrap_or_default(),
        payload.title,
        row.get(3).unwrap_or_default(),
        now,
    )
    .await?;
    Ok(Json(dto))
}

pub async fn list_messages(
    State(state): State<AppState>,
    claims: Claims,
    Path(thread_id): Path<String>,
    Query(pagination): Query<ListPaginationQuery>,
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

    touch_user_presence(&state, &claims.sub).await;
    let reaction_map = load_reactions_for_thread(&state, &thread_id, &claims.sub).await?;

    let (limit, offset) = parse_list_pagination(&pagination, 200, 500);

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
             ORDER BY m.created_at ASC
             LIMIT ?2 OFFSET ?3",
            (thread_id.clone(), limit, offset),
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
        let mid: String = row.get(0).unwrap_or_default();
        out.push(ChatMessageDto {
            id: mid.clone(),
            thread_id: row.get(1).unwrap_or_default(),
            sender_user_id: row.get(2).unwrap_or_default(),
            body: row.get(3).unwrap_or_default(),
            created_at: row.get(4).unwrap_or_default(),
            sender_username: row.get(5).ok(),
            sender_photo_url: trim_opt_url(photo_raw),
            reactions: reaction_map.get(&mid).cloned().unwrap_or_default(),
        });
    }

    state
        .db
        .execute(
            "INSERT INTO chat_reads (thread_id, user_id, last_read_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(thread_id, user_id) DO UPDATE SET last_read_at = excluded.last_read_at",
            (thread_id, claims.sub.clone(), Utc::now().to_rfc3339()),
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
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Treść wiadomości nie może być pusta",
        ));
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

    touch_user_presence(&state, &claims.sub).await;
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
    let conn_arc = state.db.raw().await;
    let sender_login = notifications::username_by_id(conn_arc.as_ref(), &claims.sub)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "Nadawca".to_string());
    notifications::notify_admin_broadcast(
        &state,
        "chat_message",
        "Nowa wiadomość na czacie",
        &format!(
            "Nowa wiadomość od „{}” (czat trener–zawodnik).",
            sender_login
        ),
        Some(
            serde_json::json!({ "thread_id": thread_id, "target_user_id": target_user })
                .to_string(),
        ),
    );
    let _ = write_audit_log(
        conn_arc.as_ref(),
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
        reactions: vec![],
    }))
}

/// Heartbeat obecności (status „online” w czacie, ~5 min).
pub async fn ping_presence(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<StatusCode, ApiError> {
    if !can_chat(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak dostępu"));
    }
    touch_user_presence(&state, &claims.sub).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn toggle_message_reaction(
    State(state): State<AppState>,
    claims: Claims,
    Path(message_id): Path<String>,
    Json(payload): Json<ToggleReactionRequest>,
) -> Result<Json<Vec<ChatReactionSummary>>, ApiError> {
    let emoji = payload.emoji.trim();
    if emoji.is_empty() || emoji.chars().count() > 16 {
        return Err(api_error(StatusCode::BAD_REQUEST, "Nieprawidłowa reakcja"));
    }
    let mut rows = state
        .db
        .query(
            "SELECT m.id, m.thread_id FROM chat_messages m
             INNER JOIN chat_threads t ON t.id = m.thread_id
             WHERE m.id = ?1 AND (t.athlete_user_id = ?2 OR t.trainer_user_id = ?2)",
            (message_id.clone(), claims.sub.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Wiadomość nie istnieje"))?;
    let thread_id: String = row.get(1).unwrap_or_default();

    let mut existing = state
        .db
        .query(
            "SELECT id FROM chat_message_reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3 LIMIT 1",
            (message_id.clone(), claims.sub.clone(), emoji.to_string()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if existing
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_some()
    {
        state
            .db
            .execute(
                "DELETE FROM chat_message_reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3",
                (message_id.clone(), claims.sub.clone(), emoji.to_string()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else {
        state
            .db
            .execute(
                "INSERT INTO chat_message_reactions (id, message_id, user_id, emoji, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    Uuid::new_v4().to_string(),
                    message_id.clone(),
                    claims.sub.clone(),
                    emoji.to_string(),
                    Utc::now().to_rfc3339(),
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let map = load_reactions_for_thread(&state, &thread_id, &claims.sub).await?;
    Ok(Json(
        map.get(&message_id).cloned().unwrap_or_default(),
    ))
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

    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
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

/// Query param dla ręcznego pruna: `?days=14` pozwala adminowi wymusić agresywniejsze cięcie
/// niż domyślne 30 dni. Walidacja: 1..=365 (poniżej ryzyko pomyłki, powyżej i tak nic by nie usunęło).
#[derive(Debug, Deserialize, Default)]
pub struct ManualPruneQuery {
    #[serde(default)]
    pub days: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ManualPruneResponse {
    pub deleted: usize,
    pub inactivity_days: i64,
    pub triggered_by: String,
}

/// `POST /api/chat/admin/prune` — natychmiastowe wywołanie czyszczenia bezczynnych wątków.
///
/// Tło: na co dzień pruner-em zajmuje się background task w `chat_cleanup`, ale gdy admin
/// chce reaktywniej posprzątać (np. przed migracją albo po incydencie spamu), endpoint
/// uruchamia tę samą funkcję ręcznie i zwraca liczbę usuniętych wątków.
pub async fn admin_prune_threads(
    State(state): State<AppState>,
    auth: RequireAdminOrSuperAdmin,
    Query(q): Query<ManualPruneQuery>,
) -> Result<Json<ManualPruneResponse>, ApiError> {
    let inactivity_days = q.days.unwrap_or(CHAT_INACTIVITY_DAYS);
    if !(1..=365).contains(&inactivity_days) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Pole `days` musi być w zakresie 1..=365",
        ));
    }

    let deleted = prune_inactive_chat_threads(&state.db, inactivity_days)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let details = serde_json::json!({
        "deleted": deleted,
        "inactivity_days": inactivity_days,
        "triggered_by": auth.0.sub,
        "reason": "manual_admin_prune",
    })
    .to_string();
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&auth.0.sub),
        Some("chat"),
        "chat",
        "manual_prune_invoked",
        None,
        None,
        Some(&details),
    )
    .await;

    Ok(Json(ManualPruneResponse {
        deleted,
        inactivity_days,
        triggered_by: auth.0.sub,
    }))
}
