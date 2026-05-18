//! Stały kod QR obecności klubu (bez wygaśnięcia) + check-in zawodnika po skanie.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
};
use chrono::{Datelike, NaiveDate, Utc, Weekday};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, RequireTrainerOrHigher};
use crate::routes::attendance::{AttendanceRecord, load_attendance_record_by_id};
use crate::state::AppState;

const SETTING_KEY: &str = "attendance_checkin_token";
pub const QR_PAYLOAD_PREFIX: &str = "SLAVIA-ATT:v1:";

#[derive(Serialize)]
pub struct AttendanceQrConfig {
    pub token: String,
    /// Pełna treść zakodowana w QR (bez daty — zawodnik skanuje w dniu treningu).
    pub payload: String,
    pub club_label: String,
}

#[derive(Deserialize)]
pub struct QrCheckinRequest {
    /// Zeskanowany payload lub sam token.
    pub payload: String,
    /// Data treningu z urządzenia (YYYY-MM-DD).
    pub session_date: String,
}

fn payload_for_token(token: &str) -> String {
    format!("{QR_PAYLOAD_PREFIX}{token}")
}

fn parse_token_from_payload(raw: &str) -> Result<String, ApiError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "Pusty kod QR"));
    }
    if let Some(rest) = s.strip_prefix(QR_PAYLOAD_PREFIX) {
        let token = rest.trim();
        if token.is_empty() {
            return Err(api_error(StatusCode::BAD_REQUEST, "Nieprawidłowy kod QR"));
        }
        return Ok(token.to_string());
    }
    Ok(s.to_string())
}

async fn read_setting_token(state: &AppState) -> Result<Option<String>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT value FROM system_settings WHERE key = ?1 LIMIT 1",
            [SETTING_KEY.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    else {
        return Ok(None);
    };
    let val: String = row
        .get(0)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let t = val.trim().to_string();
    if t.is_empty() {
        Ok(None)
    } else {
        Ok(Some(t))
    }
}

async fn write_setting_token(state: &AppState, token: &str) -> Result<(), ApiError> {
    state
        .db
        .execute(
            "INSERT INTO system_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (SETTING_KEY.to_string(), token.to_string()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

async fn ensure_token(state: &AppState) -> Result<String, ApiError> {
    if let Some(t) = read_setting_token(state).await? {
        return Ok(t);
    }
    let token = Uuid::new_v4().to_string();
    write_setting_token(state, &token).await?;
    Ok(token)
}

async fn resolve_athlete_id_for_user(state: &AppState, user_id: &str) -> Result<String, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id FROM athletes WHERE user_id = ?1 AND COALESCE(is_active, 1) = 1 LIMIT 1",
            [user_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            api_error(
                StatusCode::FORBIDDEN,
                "Brak profilu zawodnika powiązanego z kontem",
            )
        })?;
    Ok(row
        .get(0)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?)
}

fn parse_session_date(raw: &str) -> Result<String, ApiError> {
    let d = NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "session_date musi być w formacie YYYY-MM-DD",
        )
    })?;
    Ok(d.format("%Y-%m-%d").to_string())
}

/// Domyślny grafik Pn/Śr/Pt + dodatkowe dni (`extra` w recurring) + zawody kategorii trening.
async fn is_club_training_session(state: &AppState, session_date: &str) -> Result<bool, ApiError> {
    let d = NaiveDate::parse_from_str(session_date, "%Y-%m-%d")
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "Nieprawidłowa data"))?;

    let mut override_rows = state
        .db
        .query(
            "SELECT status FROM recurring_training_cancellations WHERE session_date = ?1 LIMIT 1",
            [session_date.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(row) = override_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let status: String = row.get(0).unwrap_or_default();
        let st = status.trim();
        if st == "extra" {
            return Ok(true);
        }
        if st == "cancelled" || st == "moved" {
            return Ok(false);
        }
    }

    match d.weekday() {
        Weekday::Mon | Weekday::Wed | Weekday::Fri => {}
        _ => {
            let mut comp_rows = state
                .db
                .query(
                    "SELECT id FROM competitions
                     WHERE date LIKE ?1 || '%'
                       AND status != 'cancelled'
                       AND (category = 'training' OR COALESCE(category_override, category) = 'training')
                     LIMIT 1",
                    [session_date.to_string()],
                )
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            return Ok(comp_rows
                .next()
                .await
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .is_some());
        }
    }

    Ok(true)
}

pub async fn get_attendance_qr_config(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<AttendanceQrConfig>, ApiError> {
    let token = ensure_token(&state).await?;
    Ok(Json(AttendanceQrConfig {
        payload: payload_for_token(&token),
        token: token.clone(),
        club_label: "CKS Slavia — obecność na treningu".to_string(),
    }))
}

pub async fn regenerate_attendance_qr_token(
    State(state): State<AppState>,
    auth: RequireTrainerOrHigher,
) -> Result<Json<AttendanceQrConfig>, ApiError> {
    let token = Uuid::new_v4().to_string();
    write_setting_token(&state, &token).await?;

    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&auth.0.sub),
        Some("staff"),
        "attendance",
        "qr_token_regenerate",
        None,
        None,
        None,
    )
    .await;

    Ok(Json(AttendanceQrConfig {
        payload: payload_for_token(&token),
        token,
        club_label: "CKS Slavia — obecność na treningu".to_string(),
    }))
}

pub async fn qr_checkin(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<QrCheckinRequest>,
) -> Result<Json<AttendanceRecord>, ApiError> {
    let expected = read_setting_token(&state)
        .await?
        .ok_or_else(|| api_error(StatusCode::SERVICE_UNAVAILABLE, "Kod QR nie jest skonfigurowany"))?;

    let scanned = parse_token_from_payload(&body.payload)?;
    if scanned != expected {
        return Err(api_error(StatusCode::FORBIDDEN, "Nieprawidłowy kod QR klubu"));
    }

    let session_date = parse_session_date(&body.session_date)?;
    if !is_club_training_session(&state, &session_date).await? {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "W tym dniu nie ma zaplanowanego treningu klubowego",
        ));
    }

    let athlete_id = resolve_athlete_id_for_user(&state, &claims.sub).await?;
    let now = Utc::now().to_rfc3339();

    let mut existing = state
        .db
        .query(
            "SELECT id FROM attendance_records WHERE athlete_id = ?1 AND session_date = ?2 LIMIT 1",
            (athlete_id.clone(), session_date.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let record_id = if let Some(row) = existing
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id: String = row
            .get(0)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state
            .db
            .execute(
                "UPDATE attendance_records SET status = 'obecny', source_role = 'athlete_qr',
                 verification_state = 'verified', verified_by = ?1, note = NULL, updated_at = ?2
                 WHERE id = ?3",
                (claims.sub.clone(), now.clone(), id.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        id
    } else {
        let id = Uuid::new_v4().to_string();
        state
            .db
            .execute(
                "INSERT INTO attendance_records (
                    id, athlete_id, session_date, status, source_role, created_by, verified_by,
                    verification_state, note, created_at, updated_at
                ) VALUES (?1, ?2, ?3, 'obecny', 'athlete_qr', ?4, ?4, 'verified', NULL, ?5, ?5)",
                (
                    id.clone(),
                    athlete_id.clone(),
                    session_date.clone(),
                    claims.sub.clone(),
                    now.clone(),
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        id
    };

    let rec = load_attendance_record_by_id(&state, &record_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Brak wpisu po zapisie"))?;

    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&claims.sub),
        Some("athlete"),
        "attendance",
        "qr_checkin",
        Some("athlete"),
        Some(&athlete_id),
        Some(
            &serde_json::json!({
                "session_date": session_date,
                "record_id": record_id
            })
            .to_string(),
        ),
    )
    .await;

    Ok(Json(rec))
}
