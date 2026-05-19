//! Upload i usuwanie mediów klubowych (galeria, blog, ogłoszenia) w repozytorium Slavia-cms przez GitHub Contents API.
//!
//! Awatary i zdjęcia zawodników pozostają na Cloudinary (`routes/upload.rs`).

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const DEFAULT_CMS_REPO: &str = "JakubGawron1/Slavia-cms";
const DEFAULT_CMS_BRANCH: &str = "main";
const DEFAULT_CMS_MEDIA_ROOT: &str = "media";

#[derive(Debug, Clone)]
pub struct CmsConfig {
    pub repo: String,
    pub branch: String,
    pub media_root: String,
    pub token: Option<String>,
}

pub fn github_token() -> Option<String> {
    ["GITHUB_TOKEN", "GITHUB_API_TOKEN"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn cms_config() -> CmsConfig {
    let repo = std::env::var("SLAVIA_CMS_REPO")
        .or_else(|_| std::env::var("GITHUB_CMS_REPO"))
        .unwrap_or_else(|_| DEFAULT_CMS_REPO.to_string())
        .trim()
        .to_string();
    let branch = std::env::var("SLAVIA_CMS_BRANCH")
        .unwrap_or_else(|_| DEFAULT_CMS_BRANCH.to_string())
        .trim()
        .to_string();
    let media_root = std::env::var("SLAVIA_CMS_MEDIA_ROOT")
        .unwrap_or_else(|_| DEFAULT_CMS_MEDIA_ROOT.to_string())
        .trim()
        .trim_matches('/')
        .to_string();
    CmsConfig {
        repo,
        branch,
        media_root,
        token: github_token(),
    }
}

/// Gotowe do zapisu w GitHub (repo + PAT z scope `repo`).
pub fn cms_upload_ready(cfg: &CmsConfig) -> bool {
    !cfg.repo.is_empty() && cfg.repo.contains('/') && cfg.token.is_some()
}

/// Ścieżka względna w repo CMS (zapis w DB, URL buduje frontend z `NUXT_PUBLIC_CMS_BASE_URL`).
pub fn cms_subdir_for_purpose(purpose: &str) -> &'static str {
    match purpose.trim().to_ascii_lowercase().as_str() {
        "gallery" | "media" => "gallery",
        "blog" | "post" | "article" => "blog",
        "announcements" | "announcement" | "ogloszenia" => "announcements",
        _ => "misc",
    }
}

pub fn is_cms_storage_path(url_or_path: &str) -> bool {
    normalize_cms_path(url_or_path).is_some()
}

/// Wyciąga ścieżkę w repo CMS z wartości w DB (np. `media/gallery/…`) lub pełnego URL raw GitHub / Pages.
pub fn normalize_cms_path(path_or_url: &str) -> Option<String> {
    let s = path_or_url.trim();
    if s.is_empty() {
        return None;
    }

    if !s.starts_with("http://") && !s.starts_with("https://") {
        let clean = s.trim_start_matches('/');
        return is_cms_relative_path(clean).then(|| clean.to_string());
    }

    let lower = s.to_ascii_lowercase();
    if lower.contains("raw.githubusercontent.com/")
        && let Some(idx) = lower.find("raw.githubusercontent.com/") {
            let rest = &s[idx + "raw.githubusercontent.com/".len()..];
            let path = rest.splitn(4, '/').nth(3)?;
            let path = path.split(['?', '#']).next().unwrap_or(path);
            if is_cms_relative_path(path) {
                return Some(path.to_string());
            }
        }

    if lower.contains("github.io/")
        && let Some(idx) = lower.find("github.io/") {
            let rest = &s[idx + "github.io/".len()..];
            let media_idx = rest.to_ascii_lowercase().find("media/")?;
            let path = &rest[media_idx..];
            let path = path.split(['?', '#']).next().unwrap_or(path);
            if is_cms_relative_path(path) {
                return Some(path.to_string());
            }
        }

    for base in cms_base_urls() {
        let base_lower = base.to_ascii_lowercase();
        if lower.starts_with(&base_lower) {
            let rest = &s[base.len()..];
            let path = rest.trim_start_matches('/');
            let path = path.split(['?', '#']).next().unwrap_or(path);
            if is_cms_relative_path(path) {
                return Some(path.to_string());
            }
        }
    }

    None
}

fn is_cms_relative_path(path: &str) -> bool {
    let p = path.trim_start_matches('/');
    p.starts_with("media/")
        || p.starts_with("gallery/")
        || p.starts_with("blog/")
        || p.starts_with("announcements/")
}

fn sanitize_filename(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("upload.bin");
    let mut out = String::new();
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out == "." {
        "upload.bin".to_string()
    } else {
        out
    }
}

fn build_repo_path(cfg: &CmsConfig, purpose: &str, filename: &str) -> String {
    let sub = cms_subdir_for_purpose(purpose);
    let safe = sanitize_filename(filename);
    let unique = format!("{}-{}", Uuid::new_v4(), safe);
    format!("{}/{}/{unique}", cfg.media_root, sub)
}

/// Odpowiedź GET Contents API — `sha` jest na poziomie głównym (nie w `content`).
#[derive(Debug, Deserialize)]
struct GhContentResponse {
    sha: Option<String>,
    #[serde(default)]
    content: Option<GhContentNode>,
}

#[derive(Debug, Deserialize)]
struct GhContentNode {
    sha: Option<String>,
}

fn contents_api_path(path: &str) -> String {
    path.split('/')
        .filter(|seg| !seg.is_empty())
        .map(urlencoding::encode)
        .collect::<Vec<_>>()
        .join("/")
}

fn cms_base_urls() -> Vec<String> {
    ["SLAVIA_CMS_BASE_URL", "NUXT_PUBLIC_CMS_BASE_URL"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[derive(Serialize)]
struct GhPutBody<'a> {
    message: &'a str,
    content: String,
    branch: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<&'a str>,
}

#[derive(Serialize)]
struct GhDeleteBody<'a> {
    message: &'a str,
    sha: &'a str,
    branch: &'a str,
}

async fn github_error_detail(res: reqwest::Response) -> String {
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    let snippet: String = body.chars().take(280).collect();
    if status == reqwest::StatusCode::FORBIDDEN {
        return "GitHub API: brak dostępu — ustaw GITHUB_TOKEN (scope repo) dla prywatnego Slavia-cms."
            .to_string();
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return "Plik lub repozytorium CMS nie znalezione na GitHub.".to_string();
    }
    if snippet.is_empty() {
        format!("GitHub API HTTP {}", status.as_u16())
    } else {
        format!("GitHub API HTTP {}: {}", status.as_u16(), snippet)
    }
}

async fn github_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<T, String> {
    let mut req = client
        .request(method, url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "Slavia-Backend-CMS")
        .header("Authorization", format!("Bearer {}", token));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req
        .send()
        .await
        .map_err(|e| format!("GitHub API: {}", e))?;
    if res.status().is_success() {
        return res.json().await.map_err(|e| e.to_string());
    }
    Err(github_error_detail(res).await)
}

async fn fetch_file_sha(
    client: &reqwest::Client,
    cfg: &CmsConfig,
    path: &str,
    token: &str,
) -> Result<Option<String>, String> {
    let encoded = contents_api_path(path);
    let url = format!(
        "https://api.github.com/repos/{}/contents/{}?ref={}",
        cfg.repo,
        encoded,
        urlencoding::encode(&cfg.branch)
    );
    let res = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "Slavia-Backend-CMS")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| format!("GitHub API: {}", e))?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(github_error_detail(res).await);
    }
    let parsed: GhContentResponse = res.json().await.map_err(|e| e.to_string())?;
    Ok(parsed
        .sha
        .or_else(|| parsed.content.and_then(|c| c.sha))
        .filter(|s| !s.is_empty()))
}

/// Wgrywa plik do repo CMS; zwraca ścieżkę względną (np. `media/gallery/uuid-foto.jpg`).
pub async fn upload_bytes(
    cfg: &CmsConfig,
    purpose: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let token = cfg
        .token
        .as_deref()
        .ok_or_else(|| "Brak GITHUB_TOKEN — upload do Slavia-cms niemożliwy.".to_string())?;
    if !cms_upload_ready(cfg) {
        return Err("Nieprawidłowa konfiguracja SLAVIA_CMS_REPO.".to_string());
    }

    let path = build_repo_path(cfg, purpose, filename);
    let client = reqwest::Client::new();
    let encoded = contents_api_path(&path);
    let url = format!(
        "https://api.github.com/repos/{}/contents/{}",
        cfg.repo, encoded
    );
    let existing_sha = fetch_file_sha(&client, cfg, &path, token).await?;
    let put = GhPutBody {
        message: &format!("Slavia CMS upload ({purpose}): {path}"),
        content: B64.encode(bytes),
        branch: &cfg.branch,
        sha: existing_sha.as_deref(),
    };
    let _: GhContentResponse = github_json(
        &client,
        reqwest::Method::PUT,
        &url,
        token,
        Some(serde_json::to_value(put).map_err(|e| e.to_string())?),
    )
    .await?;

    Ok(path)
}

/// Usuwa plik z repo CMS (best-effort przy kasowaniu wpisu galerii itd.).
pub async fn delete_path(cfg: &CmsConfig, path: &str) -> Result<(), String> {
    let token = match cfg.token.as_deref() {
        Some(t) => t,
        None => return Ok(()),
    };
    let path = path.trim().trim_start_matches('/');
    if path.is_empty() || !is_cms_storage_path(path) {
        return Ok(());
    }
    let client = reqwest::Client::new();
    let sha = match fetch_file_sha(&client, cfg, path, token).await? {
        Some(s) => s,
        None => {
            eprintln!("[cms] delete skip (brak sha na GitHub): {path} (repo={})", cfg.repo);
            return Ok(());
        }
    };
    let encoded = contents_api_path(path);
    let url = format!(
        "https://api.github.com/repos/{}/contents/{}",
        cfg.repo, encoded
    );
    let del = GhDeleteBody {
        message: &format!("Slavia CMS delete: {path}"),
        sha: &sha,
        branch: &cfg.branch,
    };
    let _: serde_json::Value = github_json(
        &client,
        reqwest::Method::DELETE,
        &url,
        token,
        Some(serde_json::to_value(del).map_err(|e| e.to_string())?),
    )
    .await?;
    Ok(())
}

pub async fn destroy_if_cms(path_or_url: &str) {
    let raw = path_or_url.trim();
    if raw.is_empty() {
        return;
    }
    let Some(path) = normalize_cms_path(raw) else {
        if raw.contains("media/") || raw.contains("slavia-cms") {
            eprintln!("[cms] delete skip (nie rozpoznano ścieżki CMS): {raw}");
        }
        return;
    };
    let cfg = cms_config();
    if cfg.token.is_none() {
        eprintln!(
            "[cms] delete skip (brak GITHUB_TOKEN): {path} — ustaw PAT ze scope `repo`"
        );
        return;
    }
    if let Err(e) = delete_path(&cfg, &path).await {
        eprintln!(
            "[cms] delete FAILED path={path} repo={} branch={}: {e}",
            cfg.repo, cfg.branch
        );
    } else {
        eprintln!("[cms] delete OK: {path}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_relative_gallery_path() {
        assert_eq!(
            normalize_cms_path("media/gallery/abc-photo.jpg").as_deref(),
            Some("media/gallery/abc-photo.jpg")
        );
    }

    #[test]
    fn normalize_raw_github_url() {
        assert_eq!(
            normalize_cms_path(
                "https://raw.githubusercontent.com/JakubGawron1/Slavia-cms/main/media/gallery/abc-photo.jpg"
            )
            .as_deref(),
            Some("media/gallery/abc-photo.jpg")
        );
    }

    #[test]
    fn normalize_github_pages_url() {
        assert_eq!(
            normalize_cms_path(
                "https://jakubgawron1.github.io/Slavia-cms/media/gallery/abc-photo.jpg"
            )
            .as_deref(),
            Some("media/gallery/abc-photo.jpg")
        );
    }

    #[test]
    fn reject_cloudinary_url() {
        assert!(normalize_cms_path("https://res.cloudinary.com/demo/image/upload/v1/x.jpg").is_none());
    }

    #[test]
    fn normalize_cms_base_url_prefix() {
        // raw.githubusercontent.com jest obsługiwany bez zmiennych środowiskowych
        assert_eq!(
            normalize_cms_path(
                "https://raw.githubusercontent.com/JakubGawron1/Slavia-cms/main/media/blog/x.jpg"
            )
            .as_deref(),
            Some("media/blog/x.jpg")
        );
    }

    #[test]
    fn parse_sha_from_github_contents_root() {
        let json = r#"{"sha":"abc123def","name":"foto.jpg","path":"media/gallery/foto.jpg"}"#;
        let parsed: GhContentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.sha.as_deref(), Some("abc123def"));
    }
}
