use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use crate::api_error::{ApiError, api_error};
use crate::dto::notifications::NotificationDto;
use crate::middleware::auth::Claims;
use crate::repos;
use crate::state::AppState;

pub async fn list_my_notifications(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Vec<NotificationDto>>, ApiError> {
    let conn_arc = state.db.raw().await;
    let list = repos::notifications::list_for_user(conn_arc.as_ref(), &claims.sub)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(list))
}

pub async fn delete_my_notification(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user_id = claims.sub.clone();
    let conn_arc = state.db.raw().await;
    let n = repos::notifications::delete_one(conn_arc.as_ref(), &id, &user_id)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if n == 0 {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Powiadomienie nie znalezione",
        ));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn mark_my_notification_read(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user_id = claims.sub.clone();
    let conn_arc = state.db.raw().await;
    let n = repos::notifications::mark_one_read(conn_arc.as_ref(), &id, &user_id)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if n == 0 {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Powiadomienie nie znalezione",
        ));
    }

    Ok(StatusCode::OK)
}

pub async fn mark_all_my_notifications_read(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<StatusCode, ApiError> {
    let conn_arc = state.db.raw().await;
    repos::notifications::mark_all_read(conn_arc.as_ref(), &claims.sub)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::OK)
}

pub async fn delete_all_my_notifications(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<StatusCode, ApiError> {
    let conn_arc = state.db.raw().await;
    repos::notifications::delete_all_for_user(conn_arc.as_ref(), &claims.sub)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
