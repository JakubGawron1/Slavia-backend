//! Pomocnicze operacje Cloudinary (usuwanie starego zasobu po podmianie URL).

use reqwest::Url;
use sha1::{Digest, Sha1};

use crate::state::AppState;

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

/// Wyciąga `public_id` z typowego `secure_url` zwrotnego z upload API (`.../upload/v123/folder/id.jpg`).
pub fn public_id_from_delivery_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if !parsed.host_str()?.contains("res.cloudinary.com") {
        return None;
    }
    let path = parsed.path();
    let idx = path.find("/upload/")?;
    let after_upload = path[idx + "/upload/".len()..].trim_matches('/');
    if after_upload.is_empty() {
        return None;
    }
    let parts: Vec<&str> = after_upload
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let mut start_idx = 0usize;
    for (idx, p) in parts.iter().enumerate() {
        if p.len() > 1
            && p.starts_with('v')
            && p[1..].chars().all(|c| c.is_ascii_digit())
        {
            start_idx = idx + 1;
            break;
        }
    }
    let mut pid = parts.get(start_idx..)?.join("/");
    if let Some(dot) = pid.rfind('.') {
        let ext = pid[dot + 1..].to_ascii_lowercase();
        if matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "bmp"
        ) {
            pid.truncate(dot);
        }
    }
    if pid.is_empty() {
        None
    } else {
        Some(pid)
    }
}

/// Usuwa obraz po URL z CDN Cloudinary (np. po wgraniu nowego avatara). Błędy są ignorowane (np. już skasowany).
pub async fn destroy_if_cloudinary(state: &AppState, delivery_url: &str, resource_type: &str) {
    let Some(public_id) = public_id_from_delivery_url(delivery_url) else {
        return;
    };
    if state.cloudinary_cloud_name.is_empty()
        || state.cloudinary_api_key.is_empty()
        || state.cloudinary_api_secret.is_empty()
    {
        return;
    }
    let timestamp = chrono::Utc::now().timestamp();
    let sign_params = vec![
        ("public_id".to_string(), public_id.clone()),
        ("timestamp".to_string(), timestamp.to_string()),
    ];
    let signature = cloudinary_signature(&sign_params, state.cloudinary_api_secret.as_str());
    let url = format!(
        "https://api.cloudinary.com/v1_1/{}/{}/destroy",
        state.cloudinary_cloud_name, resource_type
    );
    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .form(&[
            ("public_id", public_id.as_str()),
            ("timestamp", &timestamp.to_string()),
            ("signature", &signature),
            ("api_key", state.cloudinary_api_key.as_str()),
        ])
        .send()
        .await;
    let _ = res;
}
