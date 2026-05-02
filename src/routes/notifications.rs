use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use libsql::Row;
use serde::Serialize;

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct NotificationDto {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    pub created_at: String,
}

fn row_to_dto(row: &Row) -> Result<NotificationDto, libsql::Error> {
    Ok(NotificationDto {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        payload: row.get(4).ok(),
        created_at: row.get(5)?,
    })
}

pub async fn list_my_notifications(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Vec<NotificationDto>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, kind, title, body, payload, created_at FROM notifications \
             WHERE user_id = ?1 ORDER BY created_at DESC LIMIT 200",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut list = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let dto = row_to_dto(&row).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        list.push(dto);
    }

    Ok(Json(list))
}

pub async fn delete_my_notification(
    State(state): State<AppState>,
    Path(id): Path<String>,
    claims: Claims,
) -> Result<StatusCode, ApiError> {
    let n = state
        .db
        .execute(
            "DELETE FROM notifications WHERE id = ?1 AND user_id = ?2",
            (id, claims.sub),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Powiadomienie nie znalezione"));
    }

    Ok(StatusCode::NO_CONTENT)
}
