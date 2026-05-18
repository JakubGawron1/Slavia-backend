use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::{RequireSuperAdmin, RequireTrainerOrHigher};
use crate::state::AppState;

#[derive(Serialize)]
pub struct AuditLogRow {
    pub id: String,
    pub actor_user_id: Option<String>,
    pub actor_username: Option<String>,
    pub actor_role: Option<String>,
    pub category: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub details: Option<String>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct SystemMetricsDto {
    pub athletes_count: i64,
    pub active_plans_count: i64,
    pub pending_results_count: i64,
    pub unread_notifications_count: i64,
    pub recovery_checkins_7d_count: i64,
    pub recent_events: Vec<AuditLogRow>,
}

#[derive(Serialize)]
pub struct OpsEventRow {
    pub source: String,
    pub at: String,
    pub title: String,
    pub detail: String,
}

#[derive(Serialize)]
pub struct PingDto {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

/// Lekki endpoint diagnostyczny — loguje na stdout, która instancja odpowiada (np. `BACKEND_INSTANCE_LABEL`).
pub async fn ping_backend() -> Json<PingDto> {
    let instance = std::env::var("BACKEND_INSTANCE_LABEL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    println!(
        "[slavia-backend] GET /api/system/ping — ok{}",
        instance
            .as_ref()
            .map(|s| format!(" · instance={s:?}"))
            .unwrap_or_default()
    );
    Json(PingDto { ok: true, instance })
}

pub async fn list_audit_logs(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
) -> Result<Json<Vec<AuditLogRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT l.id, l.actor_user_id, l.actor_role, l.category, l.action, l.target_type, l.target_id, l.details, l.created_at, u.username
             FROM system_audit_logs l
             LEFT JOIN users u ON l.actor_user_id = u.id
             ORDER BY l.created_at DESC
             LIMIT 300",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(AuditLogRow {
            id: row.get(0).unwrap_or_default(),
            actor_user_id: row.get(1).ok(),
            actor_username: row.get(9).ok(),
            actor_role: row.get(2).ok(),
            category: row.get(3).unwrap_or_else(|_| "system".to_string()),
            action: row.get(4).unwrap_or_else(|_| "unknown".to_string()),
            target_type: row.get(5).ok(),
            target_id: row.get(6).ok(),
            details: row.get(7).ok(),
            created_at: row.get(8).unwrap_or_default(),
        });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct FeatureAdoptionRow {
    pub module_key: String,
    pub label: String,
    pub unique_users_30d: i64,
    pub events_30d: i64,
}

/// Panel superadmin: unikalni użytkownicy z audytu w ostatnich 30 dniach per moduł.
pub async fn feature_adoption_stats(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
) -> Result<Json<Vec<FeatureAdoptionRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT category, action, COUNT(DISTINCT actor_user_id) AS users, COUNT(*) AS events
             FROM system_audit_logs
             WHERE actor_user_id IS NOT NULL
               AND created_at >= datetime('now', '-30 day')
             GROUP BY category, action
             ORDER BY users DESC, events DESC
             LIMIT 80",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let category: String = row.get(0).unwrap_or_default();
        let action: String = row.get(1).unwrap_or_default();
        let users: i64 = row.get(2).unwrap_or(0);
        let events: i64 = row.get(3).unwrap_or(0);
        let module_key = format!("{category}:{action}");
        let label = format!("{category} · {action}");
        out.push(FeatureAdoptionRow {
            module_key,
            label,
            unique_users_30d: users,
            events_30d: events,
        });
    }
    Ok(Json(out))
}

pub async fn system_metrics(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<SystemMetricsDto>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM athletes WHERE is_active IS NULL OR is_active = 1",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let athletes_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM training_plans WHERE status IN ('planned','active')",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let active_plans_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query("SELECT COUNT(*) FROM results WHERE status = 'Pending'", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let pending_results_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query("SELECT COUNT(*) FROM notifications WHERE is_read = 0", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let unread_notifications_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM recovery_logs WHERE date >= date('now', '-7 day')",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let recovery_checkins_7d_count = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get::<i64>(0).ok())
        .unwrap_or(0);

    let mut rows = state
        .db
        .query(
            "SELECT l.id, l.actor_user_id, l.actor_role, l.category, l.action, l.target_type, l.target_id, l.details, l.created_at, u.username
             FROM system_audit_logs l
             LEFT JOIN users u ON l.actor_user_id = u.id
             ORDER BY l.created_at DESC
             LIMIT 20",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut recent_events = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        recent_events.push(AuditLogRow {
            id: row.get(0).unwrap_or_default(),
            actor_user_id: row.get(1).ok(),
            actor_username: row.get(9).ok(),
            actor_role: row.get(2).ok(),
            category: row.get(3).unwrap_or_else(|_| "system".to_string()),
            action: row.get(4).unwrap_or_else(|_| "unknown".to_string()),
            target_type: row.get(5).ok(),
            target_id: row.get(6).ok(),
            details: row.get(7).ok(),
            created_at: row.get(8).unwrap_or_default(),
        });
    }

    Ok(Json(SystemMetricsDto {
        athletes_count,
        active_plans_count,
        pending_results_count,
        unread_notifications_count,
        recovery_checkins_7d_count,
        recent_events,
    }))
}

pub async fn event_feed(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<OpsEventRow>>, ApiError> {
    let mut out: Vec<OpsEventRow> = Vec::new();

    let mut rows = state
        .db
        .query(
            "SELECT date, athlete_id, total, status FROM results ORDER BY date DESC LIMIT 40",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let at: String = row.get(0).unwrap_or_default();
        let athlete_id: String = row.get(1).unwrap_or_default();
        let total: f64 = row.get(2).unwrap_or(0.0);
        let status: String = row.get(3).unwrap_or_default();
        out.push(OpsEventRow {
            source: "results".to_string(),
            at: at.clone(),
            title: format!("Wynik {} kg ({})", total, status),
            detail: format!("athlete_id={}", athlete_id),
        });
    }

    let mut rows = state
        .db
        .query(
            "SELECT session_date, athlete_id, status, verification_state FROM attendance_records ORDER BY session_date DESC LIMIT 40",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let at: String = row.get(0).unwrap_or_default();
        let athlete_id: String = row.get(1).unwrap_or_default();
        let status: String = row.get(2).unwrap_or_default();
        let verification_state: String = row.get(3).unwrap_or_default();
        out.push(OpsEventRow {
            source: "attendance".to_string(),
            at: at.clone(),
            title: format!("Obecność: {} ({})", status, verification_state),
            detail: format!("athlete_id={}", athlete_id),
        });
    }

    let mut rows = state
        .db
        .query(
            "SELECT date, athlete_id, sleep_hours, readiness_level FROM recovery_logs ORDER BY date DESC LIMIT 40",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let at: String = row.get(0).unwrap_or_default();
        let athlete_id: String = row.get(1).unwrap_or_default();
        let sleep_hours: f64 = row.get(2).unwrap_or(0.0);
        let readiness_level: i64 = row.get(3).unwrap_or(0);
        out.push(OpsEventRow {
            source: "recovery".to_string(),
            at: at.clone(),
            title: format!(
                "Regeneracja: sen {}h, gotowość {}/10",
                sleep_hours, readiness_level
            ),
            detail: format!("athlete_id={}", athlete_id),
        });
    }

    out.sort_by(|a, b| b.at.cmp(&a.at));
    out.truncate(120);
    Ok(Json(out))
}

pub async fn openapi_handler() -> (axum::http::HeaderMap, String) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    let spec = include_str!("../embed/openapi.json").to_string();
    (headers, spec)
}

pub async fn db_backup_handler(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
) -> Result<Json<crate::routes::upload::UploadResponse>, ApiError> {
    use crate::DatabaseBackend;
    use std::fs;

    let path = match state.db.backend() {
        DatabaseBackend::Local(p) => p,
        DatabaseBackend::Remote { .. } => {
            return Err(api_error(
                StatusCode::NOT_IMPLEMENTED,
                "Backup bazy Turso nie jest obsługiwany przez ten endpoint. Użyj panelu Turso.",
            ));
        }
    };

    let bytes = fs::read(path).map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Błąd odczytu bazy: {}", e),
        )
    })?;

    if state.cloudinary_cloud_name.is_empty() {
        return Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Brak konfiguracji Cloudinary",
        ));
    }

    let timestamp = chrono::Utc::now().timestamp().to_string();
    let folder = "backups".to_string();
    let filename = format!("slavia-backup-{}.sqlite", timestamp);

    let client = reqwest::Client::new();
    let url = format!(
        "https://api.cloudinary.com/v1_1/{}/raw/upload",
        state.cloudinary_cloud_name
    );

    let mut form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(bytes)
            .file_name(filename)
            .mime_str("application/x-sqlite3")
            .unwrap(),
    );

    if !state.cloudinary_api_key.is_empty() && !state.cloudinary_api_secret.is_empty() {
        let sign_params = vec![
            ("folder".to_string(), folder.clone()),
            ("timestamp".to_string(), timestamp.clone()),
        ];
        let signature =
            crate::cloudinary::cloudinary_signature(&sign_params, &state.cloudinary_api_secret);
        form = form
            .text("api_key", state.cloudinary_api_key.clone())
            .text("folder", folder)
            .text("timestamp", timestamp)
            .text("signature", signature);
    } else {
        let preset = std::env::var("CLOUDINARY_UPLOAD_PRESET").unwrap_or_default();
        if preset.is_empty() {
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Brak konfiguracji signed/unsigned upload dla Cloudinary",
            ));
        }
        form = form.text("upload_preset", preset).text("folder", folder);
    }

    let res = client
        .post(url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(secure_url) = json.get("secure_url").and_then(|v| v.as_str()) {
        Ok(Json(crate::routes::upload::UploadResponse {
            url: secure_url.to_string(),
        }))
    } else {
        let msg = json
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("Cloudinary backup failed");
        Err(api_error(StatusCode::BAD_REQUEST, msg.to_string()))
    }
}

/// Ostatnie przebiegi zadań cron w tle (czas ściany, nie CPU) — widok `/superadmin/workers`.
pub async fn list_worker_cron_runs(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
) -> Json<Vec<crate::worker_metrics::WorkerCronRunDto>> {
    Json(state.worker_metrics.snapshot())
}

