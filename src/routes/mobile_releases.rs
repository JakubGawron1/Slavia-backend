use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::RequireSuperAdmin;
use crate::state::AppState;
use axum::{extract::State, Json};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct MobileReleaseInfo {
    pub version: String,
    pub download_url: String,
    pub published_at: String,
}

pub async fn get_latest_mobile_release(
    State(state): State<AppState>,
) -> Result<Json<MobileReleaseInfo>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT value FROM system_settings WHERE key = 'latest_mobile_release'",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows.next().await.unwrap_or(None) {
        let val: String = row.get(0).unwrap_or_default();
        if let Ok(info) = serde_json::from_str::<MobileReleaseInfo>(&val) {
            return Ok(Json(info));
        }
    }

    Err(api_error(StatusCode::NOT_FOUND, "Mobile release info not found. Please sync first."))
}

pub async fn sync_mobile_releases(
    State(state): State<AppState>,
    auth: RequireSuperAdmin,
) -> Result<Json<MobileReleaseInfo>, ApiError> {
    let claims = &auth.0;
    let repo = "JakubGawron1/Slavia-Mobile";
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);

    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .header("User-Agent", "Slavia-Backend")
        .send()
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, format!("GitHub API error: {}", e)))?;

    if !res.status().is_success() {
        return Err(api_error(StatusCode::BAD_GATEWAY, "Failed to fetch from GitHub"));
    }

    let json: serde_json::Value = res.json().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    
    let tag_name = json["tag_name"].as_str().unwrap_or("v0.0.0").to_string();
    let published_at = json["published_at"].as_str().unwrap_or("").to_string();
    
    let assets = json["assets"].as_array();
    let mut download_url = json["html_url"].as_str().unwrap_or("").to_string();

    if let Some(assets) = assets {
        for asset in assets {
            let name = asset["name"].as_str().unwrap_or("");
            if name.ends_with(".apk") {
                download_url = asset["browser_download_url"].as_str().unwrap_or(&download_url).to_string();
                break;
            }
        }
    }

    let info = MobileReleaseInfo {
        version: tag_name,
        download_url,
        published_at,
    };

    let serialized = serde_json::to_string(&info).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state.db.execute(
        "INSERT INTO system_settings (key, value) VALUES ('latest_mobile_release', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [serialized],
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let conn_arc = state.db.raw().await;
    let _ = crate::audit::write_audit_log(
        conn_arc.as_ref(),
        Some(&claims.sub),
        Some("SuperAdmin"),
        "system",
        "sync_mobile_releases",
        None,
        None,
        Some(&format!("Zsynchronizowano wersję: {}", info.version)),
    )
    .await;

    Ok(Json(info))
}
