//! Dokumenty zarządu klubu — manifest i pliki w `board/` repozytorium Slavia-cms (GitHub).

use std::collections::HashMap;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::Response,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::cms_github::{self, CmsConfig};
use crate::cms_sanitize::sanitize_cms_html;
use crate::middleware::auth::{
    Claims, RequireBoardDocsFullAccessOrSuperAdmin, RequireBoardOrSuperAdmin,
    claims_has_board_docs_full_access,
};
use crate::state::AppState;

const _: &str = ""; // manifest path via cms_github::board_manifest_repo_path()

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardDocumentVersion {
    pub version_no: i64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator_params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardDocumentEntry {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by_username: Option<String>,
    #[serde(default)]
    pub latest_version_no: i64,
    #[serde(default)]
    pub versions: Vec<BoardDocumentVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BoardCustomType {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BoardDocumentsManifest {
    #[serde(default)]
    pub documents: Vec<BoardDocumentEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_types: Vec<BoardCustomType>,
}

#[derive(Serialize)]
pub struct BoardDocsStatusDto {
    pub repo: String,
    pub branch: String,
    pub board_root: String,
    pub token_configured: bool,
    pub board_docs_ready: bool,
    pub manifest_path: String,
}

#[derive(Serialize)]
pub struct BoardPreviewDto {
    pub mime_type: String,
    pub edit_mode: &'static str,
}

#[derive(Deserialize)]
pub struct PatchContentBody {
    pub content: String,
}

#[derive(Deserialize)]
pub struct GenerateBoardDocRequest {
    pub kind: String,
    #[serde(default)]
    pub save_to_repo: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub meeting_date: Option<String>,
    #[serde(default)]
    pub competition_id: Option<String>,
}

#[derive(Deserialize)]
pub struct SaveBoardDocRequest {
    pub title: String,
    pub doc_type: String,
    pub folder: String,
    pub filename: String,
    pub content: String,
    pub mime_type: Option<String>,
}

#[derive(Deserialize)]
pub struct DeleteBoardDocRequest {
    pub id: String,
}

#[derive(Serialize)]
pub struct GenerateBoardDocResponse {
    pub content: String,
    pub mime_type: String,
    pub filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<BoardDocumentEntry>,
}

const BOARD_TEMPLATES_EMBED_JSON: &str = include_str!("../embed/board-templates.json");

#[derive(Debug, Deserialize)]
struct BoardTemplateEmbedEntry {
    mime: String,
    content: String,
}

fn board_embed_template(doc_type: &str) -> Option<(Vec<u8>, String)> {
    let map: HashMap<String, BoardTemplateEmbedEntry> =
        serde_json::from_str(BOARD_TEMPLATES_EMBED_JSON).ok()?;
    let entry = map.get(doc_type)?;
    Some((entry.content.as_bytes().to_vec(), entry.mime.clone()))
}

fn manifest_path() -> String {
    cms_github::board_manifest_repo_path()
}

fn board_not_ready_err() -> ApiError {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Slavia-cms (board/) nie jest skonfigurowane — ustaw GITHUB_TOKEN (scope repo) i SLAVIA_CMS_REPO.",
    )
}

fn ensure_board_ready(cfg: &CmsConfig) -> Result<(), ApiError> {
    if cms_github::board_docs_ready(cfg) {
        Ok(())
    } else {
        Err(board_not_ready_err())
    }
}

async fn load_manifest(cfg: &CmsConfig) -> Result<BoardDocumentsManifest, ApiError> {
    ensure_board_ready(cfg)?;
    let path = manifest_path();
    match cms_github::read_repo_file_bytes(cfg, &path).await {
        Ok((bytes, _)) => {
            let text = String::from_utf8(bytes).map_err(|e| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("Manifest UTF-8: {e}"))
            })?;
            serde_json::from_str(&text).map_err(|e| {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Nieprawidłowy _manifest.json: {e}"),
                )
            })
        }
        Err(e) if e.contains("nie znaleziony") || e.contains("NOT_FOUND") || e.contains("404") => {
            Ok(BoardDocumentsManifest::default())
        }
        Err(e) => Err(api_error(StatusCode::BAD_GATEWAY, e)),
    }
}

async fn save_manifest(cfg: &CmsConfig, manifest: &BoardDocumentsManifest) -> Result<(), ApiError> {
    ensure_board_ready(cfg)?;
    let mut m = manifest.clone();
    m.updated_at = Some(Utc::now().to_rfc3339());
    let json = serde_json::to_string_pretty(&m).map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let path = manifest_path();
    cms_github::write_repo_file_bytes(
        cfg,
        &path,
        json.as_bytes(),
        "Slavia board: update _manifest.json",
    )
    .await
    .map_err(|e| api_error(StatusCode::BAD_GATEWAY, e))?;
    Ok(())
}

fn find_doc<'a>(manifest: &'a BoardDocumentsManifest, id: &str) -> Option<&'a BoardDocumentEntry> {
    manifest.documents.iter().find(|d| d.id == id)
}

fn find_doc_mut<'a>(
    manifest: &'a mut BoardDocumentsManifest,
    id: &str,
) -> Option<&'a mut BoardDocumentEntry> {
    manifest.documents.iter_mut().find(|d| d.id == id)
}

async fn username_for_user(state: &AppState, user_id: &str) -> String {
    let mut rows = state
        .db
        .query("SELECT username FROM users WHERE id = ?1 LIMIT 1", [user_id.to_string()])
        .await
        .ok();
    if let Some(ref mut r) = rows {
        if let Ok(Some(row)) = r.next().await {
            if let Ok(u) = row.get::<String>(0) {
                return u;
            }
        }
    }
    user_id.to_string()
}

fn edit_mode_for_mime(mime: &str) -> &'static str {
    let m = mime.to_ascii_lowercase();
    if m.starts_with("text/csv")
        || m.starts_with("text/html")
        || m.starts_with("text/plain")
        || m.contains("json")
    {
        "native"
    } else {
        "download_only"
    }
}

fn folder_repo_path(folder: &str, filename: &str) -> String {
    let root = cms_github::board_docs_root();
    let folder = folder.trim().trim_matches('/');
    let filename = filename.trim().trim_matches('/');
    if folder.is_empty() {
        format!("{root}/{filename}")
    } else {
        format!("{root}/{folder}/{filename}")
    }
}

pub async fn board_docs_status(
    _auth: RequireBoardOrSuperAdmin,
) -> Result<Json<BoardDocsStatusDto>, ApiError> {
    let cfg = cms_github::cms_config();
    Ok(Json(BoardDocsStatusDto {
        repo: cfg.repo.clone(),
        branch: cfg.branch.clone(),
        board_root: cms_github::board_docs_root(),
        token_configured: cfg.token.is_some(),
        board_docs_ready: cms_github::board_docs_ready(&cfg),
        manifest_path: manifest_path(),
    }))
}

pub async fn list_board_documents(
    _auth: RequireBoardOrSuperAdmin,
) -> Result<Json<BoardDocumentsManifest>, ApiError> {
    let cfg = cms_github::cms_config();
    let manifest = load_manifest(&cfg).await?;
    Ok(Json(manifest))
}

pub async fn get_board_template(
    _auth: RequireBoardOrSuperAdmin,
    Path(doc_type): Path<String>,
) -> Result<Response, ApiError> {
    let cfg = cms_github::cms_config();
    let root = cms_github::board_docs_root();
    let safe_type = doc_type
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();
    if safe_type.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Invalid doc_type"));
    }

    if cms_github::board_docs_ready(&cfg) {
        let candidates = [
            (format!("{root}/templates/{safe_type}.html"), "text/html; charset=utf-8"),
            (format!("{root}/templates/{safe_type}.csv"), "text/csv; charset=utf-8"),
            (format!("{root}/templates/{safe_type}.txt"), "text/plain; charset=utf-8"),
            (format!("{root}/templates/{safe_type}.md"), "text/plain; charset=utf-8"),
        ];
        for (path, mime) in candidates {
            match cms_github::read_repo_file_bytes(&cfg, &path).await {
                Ok((bytes, _)) => {
                    return Ok(Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, mime)
                        .header("X-Slavia-Template-Source", "repo")
                        .body(Body::from(bytes))
                        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
                }
                Err(e) if e.contains("nie znaleziony") || e.contains("404") => continue,
                Err(e) => return Err(api_error(StatusCode::BAD_GATEWAY, e)),
            }
        }
    }

    if let Some((bytes, mime)) = board_embed_template(&safe_type) {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header("X-Slavia-Template-Source", "embed")
            .body(Body::from(bytes))
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }

    Err(api_error(
        StatusCode::NOT_FOUND,
        "Brak szablonu w repo i w katalogu wbudowanym.",
    ))
}

pub async fn get_board_document(
    _auth: RequireBoardOrSuperAdmin,
    Path(id): Path<String>,
) -> Result<Json<BoardDocumentEntry>, ApiError> {
    let cfg = cms_github::cms_config();
    let manifest = load_manifest(&cfg).await?;
    let doc = find_doc(&manifest, &id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Document not found"))?
        .clone();
    Ok(Json(doc))
}

pub async fn get_board_document_content(
    _auth: RequireBoardOrSuperAdmin,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let cfg = cms_github::cms_config();
    let manifest = load_manifest(&cfg).await?;
    let doc = find_doc(&manifest, &id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Document not found"))?;
    let repo_path = doc
        .repo_path
        .as_deref()
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Brak ścieżki pliku"))?;
    let (bytes, _) = cms_github::read_repo_file_bytes(&cfg, repo_path)
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, e))?;
    let mime = doc
        .mime_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .body(Body::from(bytes))
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?)
}

pub async fn get_board_document_preview(
    _auth: RequireBoardOrSuperAdmin,
    Path(id): Path<String>,
) -> Result<Json<BoardPreviewDto>, ApiError> {
    let cfg = cms_github::cms_config();
    let manifest = load_manifest(&cfg).await?;
    let doc = find_doc(&manifest, &id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Document not found"))?;
    let mime = doc
        .mime_type
        .as_deref()
        .unwrap_or("application/octet-stream")
        .to_string();
    Ok(Json(BoardPreviewDto {
        mime_type: mime.clone(),
        edit_mode: edit_mode_for_mime(&mime),
    }))
}

pub async fn patch_board_document_content(
    State(state): State<AppState>,
    auth: RequireBoardDocsFullAccessOrSuperAdmin,
    Path(id): Path<String>,
    Json(body): Json<PatchContentBody>,
) -> Result<Json<BoardDocumentEntry>, ApiError> {
    let cfg = cms_github::cms_config();
    let mut manifest = load_manifest(&cfg).await?;
    let doc_snapshot = find_doc(&manifest, &id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Document not found"))?
        .clone();
    let repo_path = doc_snapshot
        .repo_path
        .clone()
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Brak ścieżki pliku"))?;
    let mime = doc_snapshot
        .mime_type
        .as_deref()
        .unwrap_or("text/plain")
        .to_ascii_lowercase();

    let content_bytes = if mime.contains("html") {
        sanitize_cms_html(&body.content).into_bytes()
    } else {
        body.content.into_bytes()
    };

    let git_sha = cms_github::write_repo_file_bytes(
        &cfg,
        &repo_path,
        &content_bytes,
        &format!("Slavia board: edit {id}"),
    )
    .await
    .map_err(|e| api_error(StatusCode::BAD_GATEWAY, e))?;

    let username = username_for_user(&state, &auth.0.sub).await;
    let now = Utc::now().to_rfc3339();
    let doc = find_doc_mut(&mut manifest, &id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Document not found"))?;
    let next_ver = doc.latest_version_no + 1;
    doc.latest_version_no = next_ver;
    doc.updated_at = Some(now.clone());
    doc.versions.push(BoardDocumentVersion {
        version_no: next_ver,
        created_at: now,
        created_by: Some(auth.0.sub.clone()),
        created_by_username: Some(username),
        edit_source: Some("native".to_string()),
        generator_params: None,
        note: None,
        git_sha: Some(git_sha),
    });
    save_manifest(&cfg, &manifest).await?;
    let updated = find_doc(&manifest, &id).cloned().unwrap();
    Ok(Json(updated))
}

pub async fn save_board_document(
    State(state): State<AppState>,
    auth: RequireBoardDocsFullAccessOrSuperAdmin,
    Json(body): Json<SaveBoardDocRequest>,
) -> Result<Json<BoardDocumentEntry>, ApiError> {
    let entry = persist_board_document(&state, &auth.0, body).await?;
    Ok(Json(entry))
}

async fn persist_board_document(
    state: &AppState,
    claims: &Claims,
    body: SaveBoardDocRequest,
) -> Result<BoardDocumentEntry, ApiError> {
    let cfg = cms_github::cms_config();
    let mut manifest = load_manifest(&cfg).await?;
    let repo_path = folder_repo_path(&body.folder, &body.filename);
    let mime = body
        .mime_type
        .as_deref()
        .unwrap_or("text/plain")
        .to_string();
    let content_bytes = if mime.to_ascii_lowercase().contains("html") {
        sanitize_cms_html(&body.content).into_bytes()
    } else {
        body.content.into_bytes()
    };

    let git_sha = cms_github::write_repo_file_bytes(
        &cfg,
        &repo_path,
        &content_bytes,
        &format!("Slavia board: save {}", body.title),
    )
    .await
    .map_err(|e| api_error(StatusCode::BAD_GATEWAY, e))?;

    let username = username_for_user(state, &claims.sub).await;
    let now = Utc::now().to_rfc3339();
    let id = format!("doc-{}", Uuid::new_v4());
    let entry = BoardDocumentEntry {
        id: id.clone(),
        title: body.title,
        doc_type: Some(body.doc_type),
        folder: Some(body.folder),
        repo_path: Some(repo_path),
        mime_type: Some(mime),
        updated_at: Some(now.clone()),
        created_at: Some(now.clone()),
        created_by_username: Some(username.clone()),
        latest_version_no: 1,
        versions: vec![BoardDocumentVersion {
            version_no: 1,
            created_at: now,
            created_by: Some(claims.sub.clone()),
            created_by_username: Some(username),
            edit_source: Some("upload".to_string()),
            generator_params: None,
            note: None,
            git_sha: Some(git_sha),
        }],
    };
    manifest.documents.push(entry.clone());
    save_manifest(&cfg, &manifest).await?;
    Ok(entry)
}

pub async fn delete_board_document(
    auth: RequireBoardDocsFullAccessOrSuperAdmin,
    Json(body): Json<DeleteBoardDocRequest>,
) -> Result<StatusCode, ApiError> {
    let _ = auth;
    let cfg = cms_github::cms_config();
    let mut manifest = load_manifest(&cfg).await?;
    let pos = manifest
        .documents
        .iter()
        .position(|d| d.id == body.id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Document not found"))?;
    manifest.documents.remove(pos);
    save_manifest(&cfg, &manifest).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn build_meeting_report_csv(
    state: &AppState,
    meeting_date: Option<&str>,
) -> Result<String, ApiError> {
    let date_filter = meeting_date.unwrap_or("");
    let mut sql = String::from(
        "SELECT a.full_name, ar.status, ar.session_date FROM attendance_records ar \
         JOIN athletes a ON a.id = ar.athlete_id WHERE 1=1",
    );
    let mut params: Vec<String> = Vec::new();
    if !date_filter.is_empty() {
        sql.push_str(" AND ar.session_date = ?");
        params.push(date_filter.to_string());
    }
    sql.push_str(" ORDER BY a.full_name ASC LIMIT 500");
    let mut rows = state
        .db
        .query(&sql, params)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut lines = vec!["Zawodnik;Status;Data sesji".to_string()];
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let name: String = row.get(0).unwrap_or_default();
        let status: String = row.get(1).unwrap_or_default();
        let date: String = row.get(2).unwrap_or_default();
        lines.push(format!("\"{name}\";\"{status}\";\"{date}\""));
    }
    Ok(format!("\u{feff}{}", lines.join("\n")))
}

async fn build_start_list_csv(
    state: &AppState,
    competition_id: &str,
) -> Result<(String, String), ApiError> {
    let mut comp_rows = state
        .db
        .query(
            "SELECT title FROM competitions WHERE id = ?1",
            [competition_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let comp_title = if let Some(row) = comp_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        row.get(0).unwrap_or_else(|_| competition_id.to_string())
    } else {
        return Err(api_error(StatusCode::NOT_FOUND, "Competition not found"));
    };

    let mut rows = state
        .db
        .query(
            "SELECT a.full_name, a.weight_category, a.bodyweight FROM competition_participants cp \
             JOIN athletes a ON a.id = cp.athlete_id WHERE cp.competition_id = ?1 \
             ORDER BY a.full_name ASC",
            [competition_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut lines = vec![format!("# {comp_title}"), "Lp.;Zawodnik;Kategoria;Waga (kg)".to_string()];
    let mut n = 1i64;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let name: String = row.get(0).unwrap_or_default();
        let cat: String = row.get(1).unwrap_or_default();
        let bw: f64 = row.get(2).unwrap_or(0.0);
        lines.push(format!("{n};\"{name}\";\"{cat}\";{bw}"));
        n += 1;
    }
    let slug = competition_id.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    Ok((format!("\u{feff}{}", lines.join("\n")), slug))
}

pub async fn generate_board_document(
    State(state): State<AppState>,
    auth: RequireBoardOrSuperAdmin,
    Json(body): Json<GenerateBoardDocRequest>,
) -> Result<Json<GenerateBoardDocResponse>, ApiError> {
    let kind = body.kind.to_ascii_lowercase();
    let (content, filename, doc_type, folder, title) = match kind.as_str() {
        "meeting_report" => {
            let csv = build_meeting_report_csv(&state, body.meeting_date.as_deref()).await?;
            let date = body
                .meeting_date
                .as_deref()
                .unwrap_or("raport")
                .replace('-', "");
            (
                csv,
                format!("raport-zebranie-{date}.csv"),
                "admin_board_meeting_protocol".to_string(),
                "meeting-reports".to_string(),
                body.title
                    .unwrap_or_else(|| "Raport na zebranie".to_string()),
            )
        }
        "competition_start_list" => {
            if !claims_has_board_docs_full_access(&auth.0) {
                return Err(api_error(
                    StatusCode::FORBIDDEN,
                    "Listy startowe wymagają pełnego dostępu zarządu.",
                ));
            }
            let comp_id = body.competition_id.as_deref().ok_or_else(|| {
                api_error(StatusCode::BAD_REQUEST, "competition_id is required")
            })?;
            let (csv, slug) = build_start_list_csv(&state, comp_id).await?;
            (
                csv,
                format!("lista-startowa-{slug}.csv"),
                "competition_start_list".to_string(),
                "start-lists".to_string(),
                body.title.unwrap_or_else(|| "Lista startowa".to_string()),
            )
        }
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Unknown generator kind (meeting_report | competition_start_list)",
            ))
        }
    };

    let mut saved_doc = None;
    if body.save_to_repo && claims_has_board_docs_full_access(&auth.0) {
        let entry = persist_board_document(
            &state,
            &auth.0,
            SaveBoardDocRequest {
                title,
                doc_type,
                folder,
                filename: filename.clone(),
                content: content.clone(),
                mime_type: Some("text/csv".to_string()),
            },
        )
        .await?;
        saved_doc = Some(entry);
    }

    Ok(Json(GenerateBoardDocResponse {
        content,
        mime_type: "text/csv".to_string(),
        filename,
        document: saved_doc,
    }))
}

#[derive(Deserialize)]
pub struct UpsertCustomTypeBody {
    pub id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub category: Option<String>,
}

pub async fn list_board_document_types(
    _auth: RequireBoardOrSuperAdmin,
) -> Result<Json<Vec<BoardCustomType>>, ApiError> {
    let cfg = cms_github::cms_config();
    let manifest = load_manifest(&cfg).await?;
    Ok(Json(manifest.custom_types))
}

pub async fn upsert_board_document_type(
    auth: RequireBoardDocsFullAccessOrSuperAdmin,
    Json(body): Json<UpsertCustomTypeBody>,
) -> Result<Json<BoardCustomType>, ApiError> {
    let _ = auth;
    let cfg = cms_github::cms_config();
    let mut manifest = load_manifest(&cfg).await?;
    let id = body.id.unwrap_or_else(|| {
        format!(
            "custom_{}",
            body.label
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>()
        )
    });
    let entry = BoardCustomType {
        id: id.clone(),
        label: body.label,
        category: body.category,
    };
    if let Some(pos) = manifest.custom_types.iter().position(|t| t.id == id) {
        manifest.custom_types[pos] = entry.clone();
    } else {
        manifest.custom_types.push(entry.clone());
    }
    save_manifest(&cfg, &manifest).await?;
    Ok(Json(entry))
}
