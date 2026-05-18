use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::Utc;
use libsql::Row;
use serde::Deserialize;
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, RequireAdminOrSuperAdmin};
use crate::models::GalleryPhoto;
use crate::sql_row;
use crate::state::AppState;

fn infer_resource_type(media_type: &str) -> &'static str {
    if media_type.eq_ignore_ascii_case("video") {
        "video"
    } else {
        "image"
    }
}

fn row_to_photo(row: &Row) -> Result<GalleryPhoto, libsql::Error> {
    Ok(GalleryPhoto {
        id: sql_row::flex_string(row, 0)?,
        image_url: sql_row::flex_string(row, 1)?,
        media_type: sql_row::flex_string(row, 2).unwrap_or_else(|_| "image".to_string()),
        caption: sql_row::opt_string(row, 3)?,
        sort_order: sql_row::opt_i64(row, 4)?.unwrap_or(0),
        published: sql_row::bool_active(row, 5)?,
        author_id: sql_row::flex_string(row, 6)?,
        created_at: sql_row::flex_string(row, 7)?,
    })
}

const COLS: &str =
    "id, image_url, media_type, caption, sort_order, published, author_id, created_at";

#[derive(Deserialize)]
pub struct CreateGalleryPhotoRequest {
    pub image_url: String,
    #[serde(default = "default_media_type")]
    pub media_type: String,
    pub caption: Option<String>,
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub published: Option<bool>,
}

fn default_media_type() -> String {
    "image".to_string()
}

#[derive(Deserialize)]
pub struct UpdateGalleryPhotoRequest {
    pub image_url: String,
    #[serde(default)]
    pub media_type: Option<String>,
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
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(
            row_to_photo(&row)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
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
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(
            row_to_photo(&row)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
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
            &format!("INSERT INTO gallery_photos ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
            (
                id.clone(),
                url.to_string(),
                payload.media_type.clone(),
                payload.caption.clone(),
                sort_order,
                pub_i,
                claims.sub.clone(),
                created_at.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&claims.sub),
        Some("admin"),
        "gallery",
        "create_media",
        Some("gallery_photo"),
        Some(&id),
        Some(
            &serde_json::json!({
                "media_type": payload.media_type,
                "published": published
            })
            .to_string(),
        ),
    )
    .await;

    Ok(Json(GalleryPhoto {
        id,
        image_url: url.to_string(),
        media_type: payload.media_type,
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
        .query(
            &format!("SELECT {COLS} FROM gallery_photos WHERE id = ?1"),
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let existing = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        row_to_photo(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Photo not found"));
    };

    let url = payload.image_url.trim();
    if url.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "image_url is required"));
    }
    let media_type = payload
        .media_type
        .clone()
        .unwrap_or(existing.media_type.clone());
    let sort_order = payload.sort_order.unwrap_or(existing.sort_order);
    let published = payload.published.unwrap_or(existing.published);
    let pub_i: i64 = if published { 1 } else { 0 };

    state
        .db
        .execute(
            "UPDATE gallery_photos SET image_url = ?1, media_type = ?2, caption = ?3, sort_order = ?4, published = ?5 WHERE id = ?6",
            (
                url.to_string(),
                media_type.clone(),
                payload.caption.clone(),
                sort_order,
                pub_i,
                id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if existing.image_url != url {
        crate::cloudinary::destroy_if_cloudinary(
            &state,
            &existing.image_url,
            infer_resource_type(&existing.media_type),
        )
        .await;
        crate::cms_github::destroy_if_cms(&existing.image_url).await;
    }
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        None,
        Some("admin"),
        "gallery",
        "update_media",
        Some("gallery_photo"),
        Some(&id),
        Some(
            &serde_json::json!({
                "media_type": media_type,
                "published": published
            })
            .to_string(),
        ),
    )
    .await;

    Ok(Json(GalleryPhoto {
        id,
        image_url: url.to_string(),
        media_type,
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
    let mut rows = state
        .db
        .query(
            &format!("SELECT {COLS} FROM gallery_photos WHERE id = ?1"),
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let existing = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        row_to_photo(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Photo not found"));
    };

    let n = state
        .db
        .execute("DELETE FROM gallery_photos WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Photo not found"));
    }
    crate::cloudinary::destroy_if_cloudinary(
        &state,
        &existing.image_url,
        infer_resource_type(&existing.media_type),
    )
    .await;
    crate::cms_github::destroy_if_cms(&existing.image_url).await;
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        None,
        Some("admin"),
        "gallery",
        "delete_media",
        Some("gallery_photo"),
        Some(&id),
        None,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
