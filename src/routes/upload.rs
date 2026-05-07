use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use crate::api_error::{api_error, ApiError};
use crate::audit::write_audit_log;
use crate::middleware::auth::Claims;
use crate::state::AppState;

#[derive(Serialize)]
pub struct UploadResponse {
    pub url: String,
}

pub async fn upload_handler(
    State(state): State<AppState>,
    claims: Claims,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, ApiError> {
    if state.cloudinary_cloud_name.is_empty() {
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Brak konfiguracji Cloudinary (CLOUDINARY_CLOUD_NAME)",
        ));
    }
    let upload_preset = std::env::var("CLOUDINARY_UPLOAD_PRESET")
        .unwrap_or_default()
        .trim()
        .to_string();
    if upload_preset.is_empty() {
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Brak konfiguracji Cloudinary (CLOUDINARY_UPLOAD_PRESET dla unsigned upload)",
        ));
    }

    let field = multipart
        .next_field()
        .await
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "No file provided"))?;

    let content_type = field
        .content_type()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let filename = field
        .file_name()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "upload.jpg".to_string());

    let data = field
        .bytes()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let resource = if content_type.starts_with("video/") {
        "video"
    } else {
        "image"
    };

    let url = format!(
        "https://api.cloudinary.com/v1_1/{}/{}/upload",
        state.cloudinary_cloud_name, resource
    );

    let mime_for_part: String = if content_type.is_empty() {
        if resource == "video" {
            "video/mp4".into()
        } else {
            "application/octet-stream".into()
        }
    } else {
        content_type.clone()
    };

    let file_bytes = data.to_vec();
    let file_part = reqwest::multipart::Part::bytes(file_bytes.clone())
        .file_name(filename.clone())
        .mime_str(&mime_for_part)
        .unwrap_or_else(|_| reqwest::multipart::Part::bytes(file_bytes).file_name(filename.clone()));

    let mut form = reqwest::multipart::Form::new().part("file", file_part);
    form = form.text("upload_preset", upload_preset);

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
        let _ = write_audit_log(
            state.db.as_ref(),
            Some(&claims.sub),
            Some("upload"),
            "upload",
            "cloudinary_upload",
            Some(resource),
            None,
            Some(
                &serde_json::json!({
                    "resource_type": resource,
                    "content_type": content_type
                })
                .to_string(),
            ),
        )
        .await;
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
