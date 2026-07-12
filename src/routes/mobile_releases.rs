use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::RequireSuperAdmin;
use crate::state::AppState;
use axum::http::StatusCode;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

const DEFAULT_MOBILE_REPO: &str = "JakubGawron1/Slavia-Mobile";

#[derive(Debug, Serialize, Deserialize)]
pub struct MobileReleaseInfo {
    pub version: String,
    pub download_url: String,
    pub published_at: String,
}

fn mobile_github_repo() -> String {
    std::env::var("MOBILE_GITHUB_REPO")
        .or_else(|_| std::env::var("NUXT_PUBLIC_MOBILE_GITHUB_REPO"))
        .unwrap_or_else(|_| DEFAULT_MOBILE_REPO.to_string())
        .trim()
        .to_string()
}

fn github_token() -> Option<String> {
    ["GITHUB_TOKEN", "GITHUB_API_TOKEN"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn github_error_detail(res: reqwest::Response) -> String {
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    let snippet: String = body.chars().take(240).collect();
    if status == StatusCode::FORBIDDEN {
        return "GitHub API: limit zapytań lub brak dostępu — ustaw GITHUB_TOKEN (scope repo) dla prywatnego repozytorium."
            .to_string();
    }
    if status == StatusCode::NOT_FOUND {
        return "Repozytorium lub lista wydań nie została znaleziona na GitHub.".to_string();
    }
    if snippet.is_empty() {
        format!("GitHub API HTTP {}", status.as_u16())
    } else {
        format!("GitHub API HTTP {}: {}", status.as_u16(), snippet)
    }
}

async fn github_get(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
) -> Result<reqwest::Response, String> {
    let mut req = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "Slavia-Backend");
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    req.send()
        .await
        .map_err(|e| format!("GitHub API error: {}", e))
}

/// `/releases/latest` pomija prerelease (np. `v0.9.5-dev`) — wtedy bierzemy pierwszy z listy.
pub async fn fetch_latest_github_release(repo: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let token = github_token();
    let latest_url = format!("https://api.github.com/repos/{}/releases/latest", repo);

    let latest_res = github_get(&client, &latest_url, token.as_deref()).await?;
    if latest_res.status().is_success() {
        return latest_res.json().await.map_err(|e| e.to_string());
    }

    let latest_status = latest_res.status();
    if latest_status != StatusCode::NOT_FOUND {
        return Err(github_error_detail(latest_res).await);
    }

    let list_url = format!(
        "https://api.github.com/repos/{}/releases?per_page=30",
        repo
    );
    let list_res = github_get(&client, &list_url, token.as_deref()).await?;
    if !list_res.status().is_success() {
        return Err(github_error_detail(list_res).await);
    }

    let list: Vec<serde_json::Value> = list_res.json().await.map_err(|e| e.to_string())?;
    list.into_iter()
        .next()
        .ok_or_else(|| "Brak wydań w repozytorium GitHub.".to_string())
}

fn release_info_from_json(json: &serde_json::Value) -> MobileReleaseInfo {
    let tag_name = json["tag_name"].as_str().unwrap_or("v0.0.0").to_string();
    let published_at = json["published_at"].as_str().unwrap_or("").to_string();

    let assets = json["assets"].as_array();
    let mut download_url = json["html_url"].as_str().unwrap_or("").to_string();

    if let Some(assets) = assets {
        for asset in assets {
            let name = asset["name"].as_str().unwrap_or("");
            if name.ends_with(".apk") {
                download_url = asset["browser_download_url"]
                    .as_str()
                    .unwrap_or(&download_url)
                    .to_string();
                break;
            }
        }
    }

    MobileReleaseInfo {
        version: tag_name,
        download_url,
        published_at,
    }
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

    Err(api_error(
        StatusCode::NOT_FOUND,
        "Brak zsynchronizowanego wydania mobilnego. Użyj „Sync Mobile Releases” w panelu SuperAdmin.",
    ))
}

pub async fn sync_mobile_releases(
    State(state): State<AppState>,
    auth: RequireSuperAdmin,
) -> Result<Json<MobileReleaseInfo>, ApiError> {
    let claims = &auth.0;
    let repo = mobile_github_repo();
    if repo.is_empty() || !repo.contains('/') {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Nieprawidłowe MOBILE_GITHUB_REPO (oczekiwany format owner/repo).",
        ));
    }

    let json = fetch_latest_github_release(&repo)
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, e))?;

    let info = release_info_from_json(&json);

    let serialized = serde_json::to_string(&info)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .db
        .execute(
            "INSERT INTO system_settings (key, value) VALUES ('latest_mobile_release', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [serialized],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let conn_arc = state.db_conn().await?;
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
