use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, claims_has_staff_access};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ListCommentsQuery {
    pub target_type: String,
    pub target_id: String,
}

#[derive(Deserialize)]
pub struct CreateCommentRequest {
    pub target_type: String,
    pub target_id: String,
    pub body: String,
}

#[derive(Serialize)]
pub struct CoachComment {
    pub id: String,
    pub target_type: String,
    pub target_id: String,
    pub body: String,
    pub author_user_id: String,
    pub created_at: String,
}

pub async fn list_comments(
    State(state): State<AppState>,
    claims: Claims,
    Query(query): Query<ListCommentsQuery>,
) -> Result<Json<Vec<CoachComment>>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Komentarze trenera tylko dla kadry",
        ));
    }
    let t = query.target_type.trim().to_string();
    let tid = query.target_id.trim().to_string();
    if t.is_empty() || tid.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "target_type i target_id są wymagane",
        ));
    }

    let mut rows = state
        .db
        .query(
            "SELECT id, target_type, target_id, body, author_user_id, created_at
             FROM coach_comments
             WHERE target_type = ?1 AND target_id = ?2
             ORDER BY created_at DESC",
            (t, tid),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(CoachComment {
            id: row.get(0).unwrap_or_default(),
            target_type: row.get(1).unwrap_or_default(),
            target_id: row.get(2).unwrap_or_default(),
            body: row.get(3).unwrap_or_default(),
            author_user_id: row.get(4).unwrap_or_default(),
            created_at: row.get(5).unwrap_or_default(),
        });
    }
    Ok(Json(out))
}

pub async fn create_comment(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<CreateCommentRequest>,
) -> Result<Json<CoachComment>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Komentarze trenera tylko dla kadry",
        ));
    }
    let target_type = payload.target_type.trim().to_string();
    let target_id = payload.target_id.trim().to_string();
    let body = payload.body.trim().to_string();
    if target_type.is_empty() || target_id.is_empty() || body.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "target_type, target_id i body są wymagane",
        ));
    }
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    state
        .db
        .execute(
            "INSERT INTO coach_comments (id, target_type, target_id, body, author_user_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id.clone(),
                target_type.clone(),
                target_id.clone(),
                body.clone(),
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
        Some("staff"),
        "comments",
        "create_comment",
        Some(&target_type),
        Some(&target_id),
        None,
    )
    .await;

    Ok(Json(CoachComment {
        id,
        target_type,
        target_id,
        body,
        author_user_id: claims.sub,
        created_at,
    }))
}
