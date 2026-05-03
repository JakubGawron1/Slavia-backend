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
use crate::models::GalleryPhoto;
use crate::sql_row;
use crate::state::AppState;

fn row_to_photo(row: &Row) -> Result<GalleryPhoto, libsql::Error> {
    Ok(GalleryPhoto {
        id: sql_row::flex_string(row, 0)?,
        image_url: sql_row::flex_string(row, 1)?,
        caption: sql_row::flex_opt_string(row, 2)?,
        sort_order: sql_row::opt_i64(row, 3)?.unwrap_or(0),
        published: sql_row::bool_active(row, 4)?,
        author_id: sql_row::flex_string(row, 5)?,
        created_at: sql_row::flex_string(row, 6)?,
    })
}

const COLS: &str = "id, image_url, caption, sort_order, published, author_id, created_at";

#[derive(Deserialize)]
pub struct CreateGalleryPhotoRequest {
    pub image_url: String,
    pub caption: Option<String>,
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub published: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateGalleryPhotoRequest {
    pub image_url: String,
    pub caption: Option<String>,
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub published: Option<bool>,
}

pub async fn list_gallery_public(
    State(state): State<AppState>,
) -> Result<Json<Vec<GalleryPhoto>>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!(
                "SELECT {COLS} FROM gallery_photos WHERE published = 1 ORDER BY sort_order ASC, created_at DESC"
            ),
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        out.push(row_to_photo(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }
    Ok(Json(out))
}

pub async fn list_gallery_manage(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<GalleryPhoto>>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("SELECT {COLS} FROM gallery_photos ORDER BY sort_order ASC, created_at DESC"),
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        out.push(row_to_photo(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }
    Ok(Json(out))
}

pub async fn create_gallery_photo(
    State(state): State<AppState>,
    claims: Claims,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<CreateGalleryPhotoRequest>,
) -> Result<Json<GalleryPhoto>, ApiError> {
    let url = payload.image_url.trim();
    if url.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "image_url is required"));
    }
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let published = payload.published.unwrap_or(true);
    let sort_order = payload.sort_order.unwrap_or(0);
    let pub_i: i64 = if published { 1 } else { 0 };

    state
        .db
        .execute(
            &format!("INSERT INTO gallery_photos ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
            (
                id.clone(),
                url.to_string(),
                payload.caption.clone(),
                sort_order,
                pub_i,
                claims.sub.clone(),
                created_at.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(GalleryPhoto {
        id,
        image_url: url.to_string(),
        caption: payload.caption,
        sort_order,
        published,
        author_id: claims.sub,
        created_at,
    }))
}

pub async fn update_gallery_photo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<UpdateGalleryPhotoRequest>,
) -> Result<Json<GalleryPhoto>, ApiError> {
    let mut rows = state
        .db
        .query(&format!("SELECT {COLS} FROM gallery_photos WHERE id = ?1"), [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let existing = if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        row_to_photo(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Photo not found"));
    };

    let url = payload.image_url.trim();
    if url.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "image_url is required"));
    }
    let sort_order = payload.sort_order.unwrap_or(existing.sort_order);
    let published = payload.published.unwrap_or(existing.published);
    let pub_i: i64 = if published { 1 } else { 0 };

    state
        .db
        .execute(
            "UPDATE gallery_photos SET image_url = ?1, caption = ?2, sort_order = ?3, published = ?4 WHERE id = ?5",
            (
                url.to_string(),
                payload.caption.clone(),
                sort_order,
                pub_i,
                id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(GalleryPhoto {
        id,
        image_url: url.to_string(),
        caption: payload.caption,
        sort_order,
        published,
        author_id: existing.author_id,
        created_at: existing.created_at,
    }))
}

pub async fn delete_gallery_photo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, ApiError> {
    let n = state
        .db
        .execute("DELETE FROM gallery_photos WHERE id = ?1", [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Photo not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
