use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use libsql::Row;
use serde::Deserialize;
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::{Claims, RequireAdminOrSuperAdmin};
use crate::models::Announcement;
use crate::notifications;
use crate::sql_row;
use crate::state::AppState;

fn row_to_ann(row: &Row) -> Result<Announcement, libsql::Error> {
    Ok(Announcement {
        id: sql_row::flex_string(row, 0)?,
        title: sql_row::flex_string(row, 1)?,
        body: sql_row::flex_string(row, 2)?,
        pinned: sql_row::bool_active(row, 3)?,
        sort_order: sql_row::opt_i64(row, 4)?.unwrap_or(0),
        published: sql_row::bool_active(row, 5)?,
        author_id: sql_row::flex_string(row, 6)?,
        created_at: sql_row::flex_string(row, 7)?,
    })
}

const COLS: &str = "id, title, body, pinned, sort_order, published, author_id, created_at";

#[derive(Deserialize)]
pub struct CreateAnnouncementRequest {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub published: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateAnnouncementRequest {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub published: Option<bool>,
}

pub async fn list_announcements_public(
    State(state): State<AppState>,
) -> Result<Json<Vec<Announcement>>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!(
                "SELECT {COLS} FROM announcements WHERE published = 1 \
                 ORDER BY pinned DESC, sort_order ASC, created_at DESC"
            ),
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        out.push(row_to_ann(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }
    Ok(Json(out))
}

pub async fn list_announcements_manage(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<Announcement>>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("SELECT {COLS} FROM announcements ORDER BY pinned DESC, sort_order ASC, created_at DESC"),
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        out.push(row_to_ann(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }
    Ok(Json(out))
}

pub async fn create_announcement(
    State(state): State<AppState>,
    claims: Claims,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<CreateAnnouncementRequest>,
) -> Result<Json<Announcement>, ApiError> {
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let published = payload.published.unwrap_or(true);
    let pinned = payload.pinned.unwrap_or(false);
    let sort_order = payload.sort_order.unwrap_or(0);
    let pub_i: i64 = if published { 1 } else { 0 };
    let pin_i: i64 = if pinned { 1 } else { 0 };
    let title = payload.title.trim().to_string();
    let body = payload.body;

    state
        .db
        .execute(
            &format!("INSERT INTO announcements ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
            (
                id.clone(),
                title.clone(),
                body.clone(),
                pin_i,
                sort_order,
                pub_i,
                claims.sub.clone(),
                created_at.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if published {
        notifications::notify_announcement_published(&state, &id, &title);
    }

    Ok(Json(Announcement {
        id,
        title,
        body,
        pinned,
        sort_order,
        published,
        author_id: claims.sub,
        created_at,
    }))
}

pub async fn update_announcement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<UpdateAnnouncementRequest>,
) -> Result<Json<Announcement>, ApiError> {
    let mut rows = state
        .db
        .query(&format!("SELECT {COLS} FROM announcements WHERE id = ?1"), [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let existing = if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        row_to_ann(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Announcement not found"));
    };

    let pinned = payload.pinned.unwrap_or(existing.pinned);
    let sort_order = payload.sort_order.unwrap_or(existing.sort_order);
    let published = payload.published.unwrap_or(existing.published);
    let pin_i: i64 = if pinned { 1 } else { 0 };
    let pub_i: i64 = if published { 1 } else { 0 };
    let title = payload.title.trim().to_string();
    let body = payload.body;

    state
        .db
        .execute(
            "UPDATE announcements SET title = ?1, body = ?2, pinned = ?3, sort_order = ?4, published = ?5 WHERE id = ?6",
            (
                title.clone(),
                body.clone(),
                pin_i,
                sort_order,
                pub_i,
                id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !existing.published && published {
        notifications::notify_announcement_published(&state, &id, &title);
    }

    Ok(Json(Announcement {
        id,
        title,
        body,
        pinned,
        sort_order,
        published,
        author_id: existing.author_id,
        created_at: existing.created_at,
    }))
}

pub async fn delete_announcement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, ApiError> {
    let n = state
        .db
        .execute("DELETE FROM announcements WHERE id = ?1", [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Announcement not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
