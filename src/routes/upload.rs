use axum::{
    Json,
    extract::{Multipart, State},
    http::StatusCode,
};
use serde::Serialize;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::cloudinary::cloudinary_signature;
use crate::middleware::auth::Claims;
use crate::state::AppState;

#[derive(Serialize)]
pub struct UploadResponse {
    pub url: String,
}

/// Cele uploadu — decydują o `public_id` i folderze w Cloudinary.
///
/// • `Avatar` — jedno zdjęcie na użytkownika; `public_id = avatars/{login}` z `overwrite=true`,
///   żeby kolejne wgranie zastąpiło poprzednie pod tym samym URL.
/// • Pozostałe (`Blog`, `Gallery`, `Athletes`, `Misc`) — każdy upload tworzy
///   **nowy zasób** (Cloudinary generuje unikalne `public_id`), pogrupowany w folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UploadPurpose {
    Avatar,
    Blog,
    Gallery,
    Athletes,
    Misc,
}

impl UploadPurpose {
    fn from_raw(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("avatar") | Some("user-avatar") | Some("profile") => Self::Avatar,
            Some("blog") | Some("post") | Some("article") => Self::Blog,
            Some("gallery") | Some("media") => Self::Gallery,
            Some("athletes") | Some("athlete") | Some("player") | Some("players") => Self::Athletes,
            _ => Self::Misc,
        }
    }

    fn folder(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::Blog => "blog",
            Self::Gallery => "gallery",
            Self::Athletes => "athletes",
            Self::Misc => "misc",
        }
    }

    /// Czy kolejne uploady z tego samego konta mają nadpisywać ten sam zasób.
    /// True tylko dla awatarów — w innych miejscach (blog, galeria, zdjęcia zawodników)
    /// każde zdjęcie musi mieć osobny URL.
    fn deterministic_public_id(self) -> bool {
        matches!(self, Self::Avatar)
    }

    fn as_audit_str(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::Blog => "blog",
            Self::Gallery => "gallery",
            Self::Athletes => "athletes",
            Self::Misc => "misc",
        }
    }
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
    let can_signed = !state.cloudinary_api_key.trim().is_empty()
        && !state.cloudinary_api_secret.trim().is_empty();
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

    // Czytamy wszystkie pola z multipart — `file` (wymagane) oraz opcjonalne `purpose`.
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut purpose_raw: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                content_type = Some(
                    field
                        .content_type()
                        .map(|s| s.to_ascii_lowercase())
                        .unwrap_or_default(),
                );
                filename = Some(
                    field
                        .file_name()
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "upload.jpg".to_string()),
                );
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                file_bytes = Some(data.to_vec());
            }
            "purpose" => {
                let txt = field
                    .text()
                    .await
                    .map_err(|e| api_error(StatusCode::BAD_REQUEST, e.to_string()))?;
                let trimmed = txt.trim();
                if !trimmed.is_empty() {
                    purpose_raw = Some(trimmed.to_string());
                }
            }
            _ => {
                // Nieznane pole — odrzuć tiha (musimy odczytać body, żeby przesunąć stream).
                let _ = field.bytes().await;
            }
        }
    }

    let file_bytes =
        file_bytes.ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "No file provided"))?;
    let filename = filename.unwrap_or_else(|| "upload.jpg".to_string());
    let content_type = content_type.unwrap_or_default();

    // Task 39: Backend limit 40MB for video, 10MB for others.
    let max_size = if content_type.starts_with("video/") {
        40 * 1024 * 1024
    } else {
        10 * 1024 * 1024
    };
    if file_bytes.len() > max_size {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!(
                "Plik jest za duży (maksymalnie {} MB)",
                max_size / (1024 * 1024)
            ),
        ));
    }

    // Task 40: Block dangerous extensions
    let ext = filename
        .split('.')
        .last()
        .unwrap_or_default()
        .to_ascii_lowercase();
    const BANNED_EXT: &[&str] = &["exe", "sh", "bat", "cmd", "msi", "bin", "com"];
    if BANNED_EXT.contains(&ext.as_str()) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Niedozwolony typ pliku (ze względów bezpieczeństwa)",
        ));
    }

    let purpose = UploadPurpose::from_raw(purpose_raw.as_deref());

    // Slug loginu używany jako stabilny `public_id` tylko dla awatarów.
    let username_slug: Option<String> = if can_signed && purpose.deterministic_public_id() {
        let mut rows = state
            .db
            .query(
                "SELECT username FROM users WHERE id = ?1 LIMIT 1",
                [claims.sub.clone()],
            )
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
        Some(if out.is_empty() {
            "user".to_string()
        } else {
            out
        })
    } else {
        None
    };

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

    let folder = purpose.folder().to_string();

    // 1) Pierwsza próba: signed (jeśli mamy klucze), w przeciwnym razie unsigned preset.
    let mut form = reqwest::multipart::Form::new().part("file", make_file_part());
    if can_signed {
        let timestamp = chrono::Utc::now().timestamp().to_string();
        // Każdy parametr poza `file`, `api_key`, `signature`, `resource_type`, `cloud_name`
        // musi być uwzględniony w podpisie (alfabetycznie, key=value, połączone `&`).
        let mut sign_params: Vec<(String, String)> = vec![
            ("folder".to_string(), folder.clone()),
            ("timestamp".to_string(), timestamp.clone()),
        ];
        if purpose.deterministic_public_id() {
            sign_params.push(("overwrite".to_string(), "true".to_string()));
            if let Some(pid) = username_slug.as_ref() {
                sign_params.push(("public_id".to_string(), pid.clone()));
            }
        }
        let signature = cloudinary_signature(&sign_params, state.cloudinary_api_secret.as_str());
        form = form
            .text("api_key", state.cloudinary_api_key.clone())
            .text("folder", folder.clone())
            .text("timestamp", timestamp)
            .text("signature", signature);
        if purpose.deterministic_public_id() {
            form = form.text("overwrite", "true");
            if let Some(pid) = username_slug.as_ref() {
                form = form.text("public_id", pid.clone());
            }
        }
    } else {
        // Unsigned: preset musi pozwalać na zapis do `folder`.
        form = form
            .text("upload_preset", upload_preset_trim.clone())
            .text("folder", folder.clone());
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
                .text("upload_preset", upload_preset_trim.clone())
                .text("folder", folder.clone());
            json = send_cloudinary(&client, &url, fallback_form).await?;
        }
    }

    if let Some(secure_url) = json.get("secure_url").and_then(|v| v.as_str()) {
        let conn_arc = state.db.raw().await;
        let _ = write_audit_log(
            conn_arc.as_ref(),
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
                    "purpose": purpose.as_audit_str(),
                    "folder": folder,
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
