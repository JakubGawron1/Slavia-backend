use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::api_error::{api_error, ApiError};
use crate::cms_github::{self, CmsConfig};
use crate::middleware::auth::RequireSuperAdmin;
use crate::state::AppState;

#[derive(Serialize)]
pub struct CmsStatusDto {
    pub repo: String,
    pub branch: String,
    pub media_root: String,
    pub board_root: String,
    pub token_configured: bool,
    pub upload_ready: bool,
    pub board_docs_ready: bool,
    pub public_base_url_hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_upload_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_upload_path: Option<String>,
}

fn status_from_config(cfg: &CmsConfig) -> CmsStatusDto {
    CmsStatusDto {
        repo: cfg.repo.clone(),
        branch: cfg.branch.clone(),
        media_root: cfg.media_root.clone(),
        board_root: cms_github::board_docs_root(),
        token_configured: cfg.token.is_some(),
        upload_ready: cms_github::cms_upload_ready(cfg),
        board_docs_ready: cms_github::board_docs_ready(cfg),
        public_base_url_hint:
            "Ustaw NUXT_PUBLIC_CMS_BASE_URL na froncie (raw.githubusercontent.com/…/main lub GitHub Pages)."
                .to_string(),
        last_upload_at: None,
        last_upload_path: None,
    }
}

pub async fn cms_status(
    _auth: RequireSuperAdmin,
    State(state): State<AppState>,
) -> Result<Json<CmsStatusDto>, ApiError> {
    let cfg = cms_github::cms_config();
    let mut dto = status_from_config(&cfg);

    let mut rows = state
        .db
        .query(
            "SELECT key, value FROM system_settings WHERE key IN ('cms_last_upload_at', 'cms_last_upload_path')",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let key: String = row.get(0).unwrap_or_default();
        let val: String = row.get(1).unwrap_or_default();
        match key.as_str() {
            "cms_last_upload_at" if !val.is_empty() => dto.last_upload_at = Some(val),
            "cms_last_upload_path" if !val.is_empty() => dto.last_upload_path = Some(val),
            _ => {}
        }
    }

    Ok(Json(dto))
}
