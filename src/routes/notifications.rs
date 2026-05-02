use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::api_error::{api_error, ApiError};
use crate::dto::notifications::NotificationDto;
use crate::middleware::auth::Claims;
use crate::repos;
use crate::state::AppState;

pub async fn list_my_notifications(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Vec<NotificationDto>>, ApiError> {
    let list = repos::notifications::list_for_user(state.db.as_ref(), &claims.sub)
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
    let n = repos::notifications::delete_one(state.db.as_ref(), &id, &user_id)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Powiadomienie nie znalezione"));
    }

    Ok(StatusCode::NO_CONTENT)
}
