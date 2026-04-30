use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;
use chrono::Utc;

use crate::state::AppState;
use crate::models::Post;
use crate::middleware::auth::{Claims, RequireAdminOrSuperAdmin};

#[derive(Deserialize)]
pub struct CreatePostRequest {
    pub title: String,
    pub content: String,
}

pub async fn list_posts(
    State(state): State<AppState>,
) -> Result<Json<Vec<Post>>, (StatusCode, String)> {
    let mut rows = state
        .db
        .query("SELECT id, title, content, author_id, created_at FROM posts ORDER BY created_at DESC", ())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut posts = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        posts.push(Post {
            id: row.get(0).unwrap(),
            title: row.get(1).unwrap(),
            content: row.get(2).unwrap(),
            author_id: row.get(3).unwrap(),
            created_at: row.get(4).unwrap(),
        });
    }

    Ok(Json(posts))
}

pub async fn create_post(
    State(state): State<AppState>,
    claims: Claims, // Extractor to get author_id
    _auth: RequireAdminOrSuperAdmin, // Authorize
    Json(payload): Json<CreatePostRequest>,
) -> Result<Json<Post>, (StatusCode, String)> {
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    
    state.db.execute(
        "INSERT INTO posts (id, title, content, author_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        (id.clone(), payload.title.clone(), payload.content.clone(), claims.sub.clone(), created_at.clone()),
    ).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Post {
        id,
        title: payload.title,
        content: payload.content,
        author_id: claims.sub,
        created_at,
    }))
}

pub async fn delete_post(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, (StatusCode, String)> {
    state.db.execute("DELETE FROM posts WHERE id = ?1", [id])
        .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_post(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Post>, (StatusCode, String)> {
    let mut rows = state.db.query("SELECT id, title, content, author_id, created_at FROM posts WHERE id = ?1", [id])
        .await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows.next().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        Ok(Json(Post {
            id: row.get(0).unwrap(),
            title: row.get(1).unwrap(),
            content: row.get(2).unwrap(),
            author_id: row.get(3).unwrap(),
            created_at: row.get(4).unwrap(),
        }))
    } else {
        Err((StatusCode::NOT_FOUND, "Post not found".to_string()))
    }
}
