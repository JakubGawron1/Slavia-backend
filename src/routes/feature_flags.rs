use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, RequireTrainerOrHigher, claims_has_staff_access};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct FeatureFlagRecord {
    pub name: String,
    pub value: bool,
    pub user_id: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ListFlagsQuery {
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertFlagRequest {
    pub value: bool,
    pub user_id: Option<String>,
}

pub async fn list_feature_flags(
    State(state): State<AppState>,
    claims: Claims,
    Query(query): Query<ListFlagsQuery>,
) -> Result<Json<Vec<FeatureFlagRecord>>, ApiError> {
    let target_user = query.user_id.unwrap_or_default().trim().to_string();
    let can_list_any = claims_has_staff_access(&claims);
    let effective_user = if target_user.is_empty() {
        None
    } else {
        if !can_list_any && claims.sub != target_user {
            return Err(api_error(
                StatusCode::FORBIDDEN,
                "Brak dostępu do flag innego użytkownika",
            ));
        }
        Some(target_user)
    };

    let mut rows = if let Some(uid) = effective_user {
        state
            .db
            .query(
                "SELECT name, value, user_id, updated_at
                 FROM feature_flags
                 WHERE user_id = ?1
                 ORDER BY name ASC",
                [uid],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        state
            .db
            .query(
                "SELECT name, value, user_id, updated_at
                 FROM feature_flags
                 WHERE user_id IS NULL
                 ORDER BY name ASC",
                (),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let value_num: i64 = row.get(1).unwrap_or(0);
        out.push(FeatureFlagRecord {
            name: row.get(0).unwrap_or_default(),
            value: value_num == 1,
            user_id: row.get(2).ok(),
            updated_at: row.get(3).unwrap_or_default(),
        });
    }
    Ok(Json(out))
}

pub async fn upsert_feature_flag(
    State(state): State<AppState>,
    RequireTrainerOrHigher(claims): RequireTrainerOrHigher,
    Path(name): Path<String>,
    Json(payload): Json<UpsertFlagRequest>,
) -> Result<Json<FeatureFlagRecord>, ApiError> {
    let flag_name = name.trim().to_lowercase();
    if flag_name.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Nazwa flagi nie może być pusta",
        ));
    }
    let target_user = payload.user_id.as_ref().map(|v| v.trim().to_string());
    if let Some(uid) = target_user.as_ref()
        && uid.is_empty() {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "user_id nie może być pusty",
            ));
        }

    let now = Utc::now().to_rfc3339();
    let value_num = if payload.value { 1i64 } else { 0i64 };
    let target_user_for_sql = target_user.clone();
    state
        .db
        .execute(
            "INSERT INTO feature_flags (name, value, user_id, updated_by, updated_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(name, user_id)
             DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
            (
                flag_name.clone(),
                value_num,
                target_user_for_sql,
                claims.sub.clone(),
                now.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .db
        .execute(
            "INSERT INTO feature_flag_events (id, name, value, user_id, actor_user_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                Uuid::new_v4().to_string(),
                flag_name.clone(),
                value_num,
                target_user.clone(),
                claims.sub.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let detail = json!({
        "name": flag_name,
        "value": payload.value,
        "user_id": target_user
    })
    .to_string();
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&claims.sub),
        Some("staff"),
        "feature_flags",
        "upsert",
        Some("feature_flag"),
        Some(&name),
        Some(&detail),
    )
    .await;

    Ok(Json(FeatureFlagRecord {
        name: flag_name,
        value: payload.value,
        user_id: payload.user_id,
        updated_at: now,
    }))
}
