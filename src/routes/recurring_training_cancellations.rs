//! Powtarzalne treningi klubowe (Pn / Śr / Pt). Domyślnie „zaplanowane” bez wiersza w bazie.
//! Kadra zapisuje wyjątki: status (`cancelled`, `moved`, …) — widoczne w kalendarzu klubu i u zawodników.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{Datelike, NaiveDate, Utc, Weekday};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::RequireTrainerOrHigher;
use crate::state::AppState;

#[derive(Serialize)]
pub struct RecurringTrainingSession {
    pub session_date: String,
    pub status: String,
}

#[derive(Deserialize)]
pub struct UpsertRecurringTrainingBody {
    pub session_date: String,
    /// Domyślnie `cancelled` (kompatybilność ze starym API). `scheduled` usuwa wpis z bazy (grafik domyślny).
    #[serde(default)]
    pub status: Option<String>,
}

fn parse_club_training_date(raw: &str) -> Result<NaiveDate, ApiError> {
    let d = NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "session_date must be YYYY-MM-DD"))?;
    match d.weekday() {
        Weekday::Mon | Weekday::Wed | Weekday::Fri => Ok(d),
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Recurring club trainings are only Mon, Wed, Fri",
        )),
    }
}

fn normalize_status(raw: &str) -> Result<&'static str, ApiError> {
    match raw.trim() {
        "scheduled" => Ok("scheduled"),
        "cancelled" => Ok("cancelled"),
        "moved" => Ok("moved"),
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "status must be scheduled, cancelled or moved",
        )),
    }
}

pub async fn list_recurring_training_cancellations(
    State(state): State<AppState>,
) -> Result<Json<Vec<RecurringTrainingSession>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT session_date, status FROM recurring_training_cancellations ORDER BY session_date ASC",
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
        let session_date: String = row
            .get(0)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let status: String = row
            .get(1)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        out.push(RecurringTrainingSession {
            session_date,
            status,
        });
    }
    Ok(Json(out))
}

pub async fn upsert_recurring_training_session(
    State(state): State<AppState>,
    auth: RequireTrainerOrHigher,
    Json(body): Json<UpsertRecurringTrainingBody>,
) -> Result<StatusCode, ApiError> {
    let d = parse_club_training_date(&body.session_date)?;
    let session_date = d.format("%Y-%m-%d").to_string();

    let status_raw = body.status.unwrap_or_else(|| "cancelled".to_string());
    let status_norm = normalize_status(&status_raw)?;

    if status_norm == "scheduled" {
        state
            .db
            .execute(
                "DELETE FROM recurring_training_cancellations WHERE session_date = ?1",
                [session_date],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(StatusCode::NO_CONTENT);
    }

    let cancelled_at = Utc::now().to_rfc3339();
    let cancelled_by = auth.0.sub.clone();

    state
        .db
        .execute(
            "INSERT INTO recurring_training_cancellations (session_date, cancelled_at, cancelled_by, status) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(session_date) DO UPDATE SET \
               cancelled_at = excluded.cancelled_at, \
               cancelled_by = excluded.cancelled_by, \
               status = excluded.status",
            (session_date, cancelled_at, cancelled_by, status_norm.to_string()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn restore_recurring_training_session(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
    Path(session_date): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session_date = session_date.trim().to_string();
    if NaiveDate::parse_from_str(&session_date, "%Y-%m-%d").is_err() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "session_date must be YYYY-MM-DD",
        ));
    }

    let n = state
        .db
        .execute(
            "DELETE FROM recurring_training_cancellations WHERE session_date = ?1",
            [session_date],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if n == 0 {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "No override for this date",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn clear_all_recurring_training_cancellations(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<serde_json::Value>, ApiError> {
    let n = state
        .db
        .execute("DELETE FROM recurring_training_cancellations", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "removed": n })))
}
