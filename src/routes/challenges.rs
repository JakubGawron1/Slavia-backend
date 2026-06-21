//! Wyzwania społecznościowe — lekkie rankingi oparte na danych już w bazie (MVP ideas #66).

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use chrono::{Datelike, Utc};
use regex::Regex;
use serde::Serialize;

use crate::api_error::{ApiError, api_error};
use crate::state::AppState;

#[derive(Debug, serde::Deserialize)]
pub struct MonthlySessionsQuery {
    /// Format `YYYY-MM`. Domyślnie bieżący miesiąc (UTC).
    pub month: Option<String>,
    /// `sessions` (domyślnie) — liczba wpisów; `tonnage` — suma kg×powt. z notatek dziennika.
    pub metric: Option<String>,
}

#[derive(Serialize)]
pub struct MonthlyTrainingSessionsRow {
    pub athlete_id: String,
    pub full_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tonnage_kg: Option<i64>,
}

#[derive(Serialize)]
pub struct MonthlyTrainingSessionsResponse {
    pub month: String,
    pub metric: &'static str,
    pub leaderboard: Vec<MonthlyTrainingSessionsRow>,
}

static TONNAGE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    const PATTERNS: &[&str] = &[
        r"(?i)(\d+)\s*serii?\s*x\s*(\d+)\s*powt\.?\s*x\s*([\d.]+)\s*kg",
        r"(?i)(\d+)\s*x\s*(\d+)\s*@\s*([\d.]+)\s*kg",
        r"(?i)(\d+)\s*x\s*(\d+)\s*x\s*([\d.]+)\s*kg",
    ];
    PATTERNS
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
});

fn strip_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Suma objętości z linii typu „5 serii x 3 powt. x 100kg” (format z planu / dziennika).
fn parse_tonnage_from_notes(notes: &str) -> i64 {
    let text = strip_html(notes);
    let mut total = 0i64;
    for re in TONNAGE_PATTERNS.iter() {
        for cap in re.captures_iter(&text) {
            let sets: f64 = cap[1].parse().unwrap_or(0.0);
            let reps: f64 = cap[2].parse().unwrap_or(0.0);
            let kg: f64 = cap[3].parse().unwrap_or(0.0);
            if sets > 0.0 && reps > 0.0 && kg > 0.0 {
                total += (sets * reps * kg).round() as i64;
            }
        }
    }
    total
}

fn resolve_metric(raw: Option<String>) -> Result<&'static str, ApiError> {
    let m = raw
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    match m.as_deref() {
        None | Some("sessions") => Ok("training_log_entries_in_month"),
        Some("tonnage") => Ok("training_log_tonnage_kg_in_month"),
        Some(_) => Err(api_error(
            StatusCode::BAD_REQUEST,
            "Invalid metric query (expected sessions or tonnage)",
        )),
    }
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

/// Publiczny ranking: aktywni zawodnicy — wpisy dziennika lub tonaż (kg×powt.) w miesiącu.
pub async fn monthly_training_sessions_leaderboard(
    State(state): State<AppState>,
    Query(q): Query<MonthlySessionsQuery>,
) -> Result<Json<MonthlyTrainingSessionsResponse>, ApiError> {
    let month = normalize_month(q.month)?;
    let metric = resolve_metric(q.metric)?;
    let prefix = format!("{}%", month);

    if metric == "training_log_tonnage_kg_in_month" {
        let mut rows = state
            .db
            .query(
                "SELECT a.id, a.full_name, t.notes \
                 FROM training_log_entries t \
                 INNER JOIN athletes a ON a.id = t.athlete_id \
                 WHERE (a.is_active IS NULL OR a.is_active = 1) \
                   AND t.session_date LIKE ?1",
                [prefix.clone()],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut by_athlete: HashMap<String, (String, i64)> = HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            let athlete_id: String = row.get(0).unwrap_or_default();
            let full_name: String = row.get(1).unwrap_or_default();
            let notes: String = row.get(2).unwrap_or_default();
            let vol = parse_tonnage_from_notes(&notes);
            if vol <= 0 {
                continue;
            }
            let entry = by_athlete
                .entry(athlete_id)
                .or_insert((full_name, 0));
            entry.1 += vol;
        }

        let mut leaderboard: Vec<MonthlyTrainingSessionsRow> = by_athlete
            .into_iter()
            .map(|(athlete_id, (full_name, tonnage_kg))| MonthlyTrainingSessionsRow {
                athlete_id,
                full_name,
                session_count: None,
                tonnage_kg: Some(tonnage_kg),
            })
            .collect();
        leaderboard.sort_by(|a, b| {
            let av = a.tonnage_kg.unwrap_or(0);
            let bv = b.tonnage_kg.unwrap_or(0);
            bv.cmp(&av).then_with(|| a.full_name.cmp(&b.full_name))
        });
        leaderboard.truncate(50);

        return Ok(Json(MonthlyTrainingSessionsResponse {
            month,
            metric,
            leaderboard,
        }));
    }

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
            session_count: Some(session_count),
            tonnage_kg: None,
        });
    }

    Ok(Json(MonthlyTrainingSessionsResponse {
        month,
        metric,
        leaderboard,
    }))
}
