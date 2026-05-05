use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::RequireSuperAdmin;
use crate::state::AppState;

#[derive(Serialize)]
pub struct AuditLogRow {
    pub id: String,
    pub actor_user_id: Option<String>,
    pub actor_role: Option<String>,
    pub category: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub details: Option<String>,
    pub created_at: String,
}

pub async fn list_audit_logs(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
) -> Result<Json<Vec<AuditLogRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, actor_user_id, actor_role, category, action, target_type, target_id, details, created_at
             FROM system_audit_logs
             ORDER BY created_at DESC
             LIMIT 300",
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
        out.push(AuditLogRow {
            id: row.get(0).unwrap_or_default(),
            actor_user_id: row.get(1).ok(),
            actor_role: row.get(2).ok(),
            category: row.get(3).unwrap_or_else(|_| "system".to_string()),
            action: row.get(4).unwrap_or_else(|_| "unknown".to_string()),
            target_type: row.get(5).ok(),
            target_id: row.get(6).ok(),
            details: row.get(7).ok(),
            created_at: row.get(8).unwrap_or_default(),
        });
    }
    Ok(Json(out))
}
