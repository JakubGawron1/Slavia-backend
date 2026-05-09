use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use libsql::Row;
use serde::Deserialize;
use uuid::Uuid;
use chrono::Utc;

use crate::api_error::{api_error, ApiError};
use crate::models::Post;
use crate::notifications;
use crate::state::AppState;
use crate::middleware::auth::{Claims, RequireAdminOrSuperAdmin};
use crate::sql_row;

fn post_from_row(row: &Row) -> Result<Post, libsql::Error> {
    Ok(Post {
        id: sql_row::flex_string(row, 0)?,
        title: sql_row::flex_string(row, 1)?,
        content: sql_row::flex_string(row, 2)?,
        author_id: sql_row::flex_string(row, 3)?,
        image_url: sql_row::flex_opt_string(row, 4)?,
        created_at: sql_row::flex_string(row, 5)?,
        published: sql_row::bool_active(row, 6)?,
    })
}

#[derive(Deserialize)]
pub struct CreatePostRequest {
    pub title: String,
    pub content: String,
    pub image_url: Option<String>,
    /// `false` = szkic (niewidoczny na liście publicznej). Domyślnie `true` dla kompatybilności.
    #[serde(default)]
    pub published: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdatePostRequest {
    pub title: String,
    pub content: String,
    pub image_url: Option<String>,
    #[serde(default)]
    pub published: Option<bool>,
}

const POST_COLUMNS: &str =
    "id, title, content, author_id, image_url, created_at, published";

/// Lista publiczna — tylko opublikowane wpisy.
pub async fn list_posts_public(
    State(state): State<AppState>,
) -> Result<Json<Vec<Post>>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!(
                "SELECT {POST_COLUMNS} FROM posts WHERE published = 1 ORDER BY created_at DESC"
            ),
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut posts = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let p = post_from_row(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        posts.push(p);
    }

    Ok(Json(posts))
}

/// Lista dla panelu admina — także szkice.
pub async fn list_posts_manage(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<Post>>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("SELECT {POST_COLUMNS} FROM posts ORDER BY created_at DESC"),
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut posts = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let p = post_from_row(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        posts.push(p);
    }

    Ok(Json(posts))
}

pub async fn create_post(
    State(state): State<AppState>,
    claims: Claims,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<CreatePostRequest>,
) -> Result<Json<Post>, ApiError> {
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let published = payload.published.unwrap_or(true);
    let pub_i: i64 = if published { 1 } else { 0 };

    state
        .db
        .execute(
            &format!(
                "INSERT INTO posts ({POST_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ),
            (
                id.clone(),
                payload.title.clone(),
                payload.content.clone(),
                claims.sub.clone(),
                payload.image_url.clone(),
                created_at.clone(),
                pub_i,
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if published {
        let conn_arc = state.db.raw().await;
        let author = notifications::username_by_id(conn_arc.as_ref(), &claims.sub)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "?".to_string());
        notifications::notify_admin_broadcast(
            &state,
            "blog_post_created",
            "Nowy wpis na blogu",
            &format!("{} opublikował(a): „{}”.", author, payload.title),
            Some(serde_json::json!({ "post_id": id.clone() }).to_string()),
        );
    }

    Ok(Json(Post {
        id,
        title: payload.title,
        content: payload.content,
        author_id: claims.sub,
        image_url: payload.image_url,
        created_at,
        published,
    }))
}

pub async fn update_post(
    State(state): State<AppState>,
    Path(id): Path<String>,
    claims: Claims,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<UpdatePostRequest>,
) -> Result<Json<Post>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("SELECT {POST_COLUMNS} FROM posts WHERE id = ?1"),
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let existing = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        post_from_row(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Post not found"));
    };

    let next_published = payload.published.unwrap_or(existing.published);
    let pub_i: i64 = if next_published { 1 } else { 0 };

    state
        .db
        .execute(
            "UPDATE posts SET title = ?1, content = ?2, image_url = ?3, published = ?4 WHERE id = ?5",
            (
                payload.title.clone(),
                payload.content.clone(),
                payload.image_url.clone(),
                pub_i,
                id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let title_old = existing.title.clone();
    let conn_arc = state.db.raw().await;
    let editor = notifications::username_by_id(conn_arc.as_ref(), &claims.sub)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "?".to_string());
    notifications::notify_admin_broadcast(
        &state,
        "blog_post_updated",
        "Zaktualizowano wpis",
        &format!(
            "{} zaktualizował(a) wpis: „{}” → „{}”.",
            editor, title_old, payload.title
        ),
        Some(serde_json::json!({ "post_id": id.clone() }).to_string()),
    );

    Ok(Json(Post {
        id,
        title: payload.title,
        content: payload.content,
        author_id: existing.author_id,
        image_url: payload.image_url,
        created_at: existing.created_at,
        published: next_published,
    }))
}

pub async fn delete_post(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, ApiError> {
    let mut rows = state
        .db
        .query("SELECT title FROM posts WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let title_opt = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        row.get::<String>(0).ok()
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Post not found"));
    };

    state
        .db
        .execute("DELETE FROM posts WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let t = title_opt.unwrap_or_else(|| "?".to_string());
    notifications::notify_admin_broadcast(
        &state,
        "blog_post_deleted",
        "Usunięto wpis",
        &format!("Usunięto wpis blogowy: „{}”.", t),
        Some(serde_json::json!({ "post_id": id }).to_string()),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Publiczny widok wpisu — wyłącznie opublikowany.
pub async fn get_post_public(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Post>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("SELECT {POST_COLUMNS} FROM posts WHERE id = ?1 AND published = 1"),
            [id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let p = post_from_row(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(p))
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "Post not found"))
    }
}

/// Pełny wpis dla panelu (szkice).
pub async fn get_post_manage(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Post>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("SELECT {POST_COLUMNS} FROM posts WHERE id = ?1"),
            [id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let p = post_from_row(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(p))
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "Post not found"))
    }
}
