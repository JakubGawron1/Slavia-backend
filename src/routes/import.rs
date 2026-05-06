use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::Claims;
use crate::models::Role;
use crate::state::AppState;

#[derive(Serialize)]
pub struct ImportRejectedSample {
    pub full_name: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct ImportResult {
    pub source: String,
    pub urls_attempted: usize,
    pub urls_fetched_ok: usize,
    pub fetch_errors: Vec<String>,
    pub rows_parsed: usize,
    pub records_matched_roster: usize,
    pub records_saved: usize,
    pub records_duplicate_skipped: usize,
    pub records_preview_importable: usize,
    #[serde(default)]
    pub rejected_samples: Vec<ImportRejectedSample>,
    /// Liczba rekordów dopasowanych do kadry klubu (jak dawniej `athletes_updated`).
    pub athletes_updated: usize,
    /// Zapisane (`dev_mode=true`) albo liczba rekordów kwalifikujących się do importu w podglądzie (`dev_mode=false`).
    pub new_results: usize,
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub dev_mode: Option<bool>,
}

pub async fn import_data_handler(
    State(_state): State<AppState>,
    claims: Claims,
    Json(_payload): Json<ImportRequest>,
) -> Result<Json<Vec<ImportResult>>, ApiError> {
    if !claims.roles.contains(&Role::SuperAdmin) {
        return Err(api_error(StatusCode::FORBIDDEN, "Only superadmin can import data"));
    }
    Err(api_error(
        StatusCode::GONE,
        "Import danych zawodników z federacji został wyłączony. Import zawodów pozostaje dostępny w kalendarzu.",
    ))
}
