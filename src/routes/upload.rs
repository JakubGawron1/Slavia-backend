use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use crate::api_error::{api_error, ApiError};
use crate::audit::write_audit_log;
use crate::cloudinary::cloudinary_signature;
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
    // Preferuj signed upload (Render ma zmienne IP → whitelisting przy unsigned presetach bywa problematyczny).
    // Fallback: unsigned upload preset (legacy).
    let can_signed = !state.cloudinary_api_key.trim().is_empty() && !state.cloudinary_api_secret.trim().is_empty();
    let upload_preset = std::env::var("CLOUDINARY_UPLOAD_PRESET")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !can_signed && upload_preset.is_empty() {
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Brak konfiguracji Cloudinary (CLOUDINARY_UPLOAD_PRESET dla unsigned upload) ani kluczy do signed upload (CLOUDINARY_API_KEY/CLOUDINARY_API_SECRET).",
        ));
    }
    let upload_preset_trim = upload_preset.trim().to_string();

    // Dla signed upload: ustaw `public_id` na login użytkownika (czytelne nazwy i stały identyfikator zasobu).
    // Jeśli login ma nietypowe znaki, sanitizujemy do `a-z0-9-`.
    let username_slug: Option<String> = if can_signed {
        let mut rows = state
            .db
            .query("SELECT username FROM users WHERE id = ?1 LIMIT 1", [claims.sub.clone()])
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let row = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "User not found"))?;
        let username: String = row.get(0).unwrap_or_else(|_| "user".to_string());
        let mut out = String::new();
        for ch in username.chars() {
            let lc = ch.to_ascii_lowercase();
            if lc.is_ascii_alphanumeric() {
                out.push(lc);
            } else if matches!(lc, '-' | '_' | '.' | ' ') {
                out.push('-');
            }
        }
        let out = out.trim_matches('-').to_string();
        Some(if out.is_empty() { "user".to_string() } else { out })
    } else {
        None
    };

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
    let make_file_part = || {
        reqwest::multipart::Part::bytes(file_bytes.clone())
            .file_name(filename.clone())
            .mime_str(&mime_for_part)
            .unwrap_or_else(|_| {
                reqwest::multipart::Part::bytes(file_bytes.clone()).file_name(filename.clone())
            })
    };

    async fn send_cloudinary(
        client: &reqwest::Client,
        url: &str,
        form: reqwest::multipart::Form,
    ) -> Result<serde_json::Value, ApiError> {
        let res = client
            .post(url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        res.json()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    }

    let client = reqwest::Client::new();

    // 1) Pierwsza próba: signed (jeśli mamy klucze), w przeciwnym razie unsigned preset.
    let mut form = reqwest::multipart::Form::new().part("file", make_file_part());
    if can_signed {
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let overwrite = "true".to_string();
        let mut sign_params = vec![
            ("overwrite".to_string(), overwrite.clone()),
            ("timestamp".to_string(), timestamp.clone()),
        ];
        if let Some(pid) = username_slug.as_ref() {
            sign_params.push(("public_id".to_string(), pid.clone()));
        }
        let signature = cloudinary_signature(&sign_params, state.cloudinary_api_secret.as_str());
        form = form
            .text("api_key", state.cloudinary_api_key.clone())
            .text("overwrite", overwrite)
            .text("timestamp", timestamp)
            .text("signature", signature);
        if let Some(pid) = username_slug.as_ref() {
            form = form.text("public_id", pid.clone());
        }
    } else {
        form = form.text("upload_preset", upload_preset_trim.clone());
    }

    let mut json: serde_json::Value = send_cloudinary(&client, &url, form).await?;

    // 2) Jeśli signed wywali się na timestamp/signature, a preset jest dostępny, spróbuj fallbacku unsigned.
    if json.get("secure_url").and_then(|v| v.as_str()).is_none()
        && can_signed
        && !upload_preset_trim.is_empty()
    {
        let msg_lc = json
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let looks_like_time_issue = msg_lc.contains("timestamp") || msg_lc.contains("signature");
        if looks_like_time_issue {
            let fallback_form = reqwest::multipart::Form::new()
                .part("file", make_file_part())
                .text("upload_preset", upload_preset_trim.clone());
            json = send_cloudinary(&client, &url, fallback_form).await?;
        }
    }

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
                    "content_type": content_type,
                    "public_id": username_slug
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
