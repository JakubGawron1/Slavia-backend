use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::Utc;
use libsql::Row;
use serde::Deserialize;
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::RequireAdminOrSuperAdmin;
use crate::models::ContactMessage;
use crate::sql_row;
use crate::state::AppState;

fn row_to_msg(row: &Row) -> Result<ContactMessage, libsql::Error> {
    Ok(ContactMessage {
        id: sql_row::flex_string(row, 0)?,
        name: sql_row::flex_string(row, 1)?,
        email: sql_row::flex_opt_string(row, 2)?,
        phone: sql_row::flex_opt_string(row, 3)?,
        message: sql_row::flex_string(row, 4)?,
        created_at: sql_row::flex_string(row, 5)?,
        is_read: sql_row::bool_active(row, 6)?,
    })
}

const COLS: &str = "id, name, email, phone, message, created_at, is_read";

#[derive(Deserialize)]
pub struct SubmitContactRequest {
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub message: String,
}

#[derive(Deserialize)]
pub struct PatchContactMessageRequest {
    #[serde(default)]
    pub is_read: Option<bool>,
}

fn client_ip_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
        })
        .unwrap_or("unknown")
        .to_string()
}

pub async fn submit_contact_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<SubmitContactRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let client_ip = client_ip_from_headers(&headers);
    if crate::distributed_throttle::reserve_contact_submit(&state, &client_ip)
        .await
        .is_err()
    {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Zbyt wiele wiadomości z tego adresu — spróbuj za chwilę.",
        ));
    }

    let name = payload.name.trim();
    let message = payload.message.trim();
    if name.is_empty() || message.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "name and message are required",
        ));
    }
    if message.len() > 8000 {
        return Err(api_error(StatusCode::BAD_REQUEST, "message too long"));
    }
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let email = payload.email.and_then(|e| {
        let t = e.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });
    let phone = payload.phone.and_then(|p| {
        let t = p.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });

    state
        .db
        .execute(
            &format!("INSERT INTO contact_messages ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
            (
                id.clone(),
                name.to_string(),
                email.clone(),
                phone.clone(),
                message.to_string(),
                created_at,
                0i64,
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": id, "ok": true })),
    ))
}

pub async fn list_contact_messages_manage(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<Vec<ContactMessage>>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("SELECT {COLS} FROM contact_messages ORDER BY is_read ASC, created_at DESC"),
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
            row_to_msg(&row)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
    }
    Ok(Json(out))
}

pub async fn patch_contact_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
    Json(payload): Json<PatchContactMessageRequest>,
) -> Result<Json<ContactMessage>, ApiError> {
    let mut rows = state
        .db
        .query(
            &format!("SELECT {COLS} FROM contact_messages WHERE id = ?1"),
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let existing = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        row_to_msg(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Message not found"));
    };

    let is_read = payload.is_read.unwrap_or(existing.is_read);
    let read_i: i64 = if is_read { 1 } else { 0 };

    state
        .db
        .execute(
            "UPDATE contact_messages SET is_read = ?1 WHERE id = ?2",
            (read_i, id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ContactMessage {
        is_read,
        ..existing
    }))
}

pub async fn delete_contact_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<StatusCode, ApiError> {
    let n = state
        .db
        .execute("DELETE FROM contact_messages WHERE id = ?1", [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Message not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
