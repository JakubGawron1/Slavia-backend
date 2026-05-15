//! Wyzwania społecznościowe — lekkie rankingi oparte na danych już w bazie (MVP ideas #66).

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use chrono::{Datelike, Utc};
use serde::Serialize;

use crate::api_error::{ApiError, api_error};
use crate::state::AppState;

#[derive(Debug, serde::Deserialize)]
pub struct MonthlySessionsQuery {
    /// Format `YYYY-MM`. Domyślnie bieżący miesiąc (UTC).
    pub month: Option<String>,
}

#[derive(Serialize)]
pub struct MonthlyTrainingSessionsRow {
    pub athlete_id: String,
    pub full_name: String,
    pub session_count: i64,
}

#[derive(Serialize)]
pub struct MonthlyTrainingSessionsResponse {
    pub month: String,
    /// Wynik wg liczby wpisów w dzienniku treningów w danym miesiącu (proxy „aktywności” — nie objętość kg).
    pub metric: &'static str,
    pub leaderboard: Vec<MonthlyTrainingSessionsRow>,
}

fn normalize_month(raw: Option<String>) -> Result<String, ApiError> {
    if let Some(m) = raw {
        let t = m.trim();
        if t.len() == 7
            && t.as_bytes()[4] == b'-'
            && t[0..4].chars().all(|c| c.is_ascii_digit())
            && t[5..7].chars().all(|c| c.is_ascii_digit())
        {
            let mo: i32 = t[5..7].parse().map_err(|_| {
                api_error(StatusCode::BAD_REQUEST, "Invalid month query (expected YYYY-MM)")
            })?;
            if !(1..=12).contains(&mo) {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Invalid month query (month out of range)",
                ));
            }
            return Ok(t.to_string());
        }
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Invalid month query (expected YYYY-MM)",
        ));
    }
    let now = Utc::now().date_naive();
    Ok(format!("{:04}-{:02}", now.year(), now.month()))
}

/// Publiczny ranking: aktywni zawodnicy z co najmniej jednym wpisem dziennika w wybranym miesiącu.
pub async fn monthly_training_sessions_leaderboard(
    State(state): State<AppState>,
    Query(q): Query<MonthlySessionsQuery>,
) -> Result<Json<MonthlyTrainingSessionsResponse>, ApiError> {
    let month = normalize_month(q.month)?;
    let prefix = format!("{}%", month);

    let mut rows = state
        .db
        .query(
            "SELECT a.id, a.full_name, COUNT(t.id) AS session_count \
             FROM training_log_entries t \
             INNER JOIN athletes a ON a.id = t.athlete_id \
             WHERE (a.is_active IS NULL OR a.is_active = 1) \
               AND t.session_date LIKE ?1 \
             GROUP BY a.id, a.full_name \
             HAVING session_count > 0 \
             ORDER BY session_count DESC, a.full_name ASC \
             LIMIT 50",
            [prefix],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut leaderboard = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let athlete_id: String = row.get(0).unwrap_or_default();
        let full_name: String = row.get(1).unwrap_or_default();
        let session_count: i64 = row.get(2).unwrap_or(0);
        leaderboard.push(MonthlyTrainingSessionsRow {
            athlete_id,
            full_name,
            session_count,
        });
    }

    Ok(Json(MonthlyTrainingSessionsResponse {
        month,
        metric: "training_log_entries_in_month",
        leaderboard,
    }))
}
