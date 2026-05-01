use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use serde::Serialize;
use sha1::{Digest, Sha1};

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::Claims;
use crate::state::AppState;

#[derive(Serialize)]
pub struct UploadResponse {
    pub url: String,
}

/// Podpis Cloudinary: SHA1(heks) z `posortowane key=value po &` + `api_secret`.
fn cloudinary_signature(params: &[(String, String)], api_secret: &str) -> String {
    let mut pairs = params.to_vec();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let string_to_sign = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");
    let mut hasher = Sha1::new();
    hasher.update(string_to_sign.as_bytes());
    hasher.update(api_secret.as_bytes());
    hex::encode(hasher.finalize())
}

pub async fn upload_handler(
    State(state): State<AppState>,
    _claims: Claims,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, ApiError> {
    if state.cloudinary_cloud_name.is_empty()
        || state.cloudinary_api_key.is_empty()
        || state.cloudinary_api_secret.is_empty()
    {
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Brak konfiguracji Cloudinary (CLOUDINARY_CLOUD_NAME / API_KEY / API_SECRET)",
        ));
    }

    let field = multipart
        .next_field()
        .await
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "No file provided"))?;

    let filename = field
        .file_name()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "upload.jpg".to_string());

    let data = field
        .bytes()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let timestamp = Utc::now().timestamp();
    let mut sign_params = vec![(
        "timestamp".to_string(),
        timestamp.to_string(),
    )];

    // Opcjonalnie: preset nadal możliwy, ale musi być w parametrach podpisu (signed preset).
    if let Ok(preset) = std::env::var("CLOUDINARY_UPLOAD_PRESET") {
        let p = preset.trim();
        if !p.is_empty() {
            sign_params.push(("upload_preset".to_string(), p.to_string()));
        }
    }

    let signature = cloudinary_signature(
        &sign_params,
        state.cloudinary_api_secret.as_str(),
    );

    let url = format!(
        "https://api.cloudinary.com/v1_1/{}/image/upload",
        state.cloudinary_cloud_name
    );

    let mut form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(data.to_vec()).file_name(filename),
    );
    form = form.text("api_key", state.cloudinary_api_key.clone());
    form = form.text("timestamp", timestamp.to_string());
    form = form.text("signature", signature);

    for (k, v) in &sign_params {
        if k != "timestamp" {
            form = form.text(k.clone(), v.clone());
        }
    }

    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(secure_url) = json.get("secure_url").and_then(|v| v.as_str()) {
        return Ok(Json(UploadResponse {
            url: secure_url.to_string(),
        }));
    }

    let msg = json
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Cloudinary error: {:?}", json));

    Err(api_error(StatusCode::BAD_REQUEST, msg))
}
