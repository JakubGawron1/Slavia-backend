use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::{Claims, RequireTrainerOrHigher};
use crate::sql_row;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateMyPaymentRequest {
    pub month: Option<String>, // YYYY-MM
    pub amount_pln: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PaymentStatusResponse {
    pub month: String,
    pub due_date: String, // YYYY-MM-10
    pub is_paid: bool,
    pub is_overdue: bool,
}

#[derive(Debug, Deserialize)]
pub struct MonthQuery {
    pub month: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AthletePaymentStatusRow {
    pub athlete_id: String,
    pub full_name: String,
    pub is_paid: bool,
}

#[derive(Debug, Serialize)]
pub struct PendingPaymentRow {
    pub id: String,
    pub athlete_id: String,
    pub athlete_name: String,
    pub month: String,
    pub amount_pln: Option<f64>,
    pub note: Option<String>,
    pub created_at: String,
    pub created_by_user_id: Option<String>,
}

fn current_month_yyyy_mm() -> String {
    let now = Utc::now().date_naive();
    format!("{:04}-{:02}", now.year(), now.month())
}

fn parse_month_yyyy_mm(s: &str) -> Result<(i32, u32), ApiError> {
    let s = s.trim();
    if s.len() != 7 || &s[4..5] != "-" {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Nieprawidłowy format miesiąca. Oczekiwano YYYY-MM.",
        ));
    }
    let y = s[0..4]
        .parse::<i32>()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "Nieprawidłowy rok w miesiącu"))?;
    let m = s[5..7]
        .parse::<u32>()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "Nieprawidłowy miesiąc"))?;
    if !(1..=12).contains(&m) {
        return Err(api_error(StatusCode::BAD_REQUEST, "Nieprawidłowy miesiąc"));
    }
    Ok((y, m))
}

fn due_date_yyyy_mm_10(month: &str) -> Result<NaiveDate, ApiError> {
    let (y, m) = parse_month_yyyy_mm(month)?;
    NaiveDate::from_ymd_opt(y, m, 10).ok_or_else(|| {
        api_error(
            StatusCode::BAD_REQUEST,
            "Nieprawidłowy miesiąc (nie da się wyliczyć terminu 10-go).",
        )
    })
}

async fn my_athlete_id(state: &AppState, claims: &Claims) -> Result<String, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id FROM athletes WHERE user_id = ?1", [claims.sub.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Nie znaleziono profilu zawodnika dla tego konta"))?;
    let id: String = row.get(0).unwrap_or_default();
    if id.trim().is_empty() {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Nie znaleziono profilu zawodnika dla tego konta",
        ));
    }
    Ok(id)
}

async fn is_month_paid_approved(state: &AppState, athlete_id: &str, month: &str) -> Result<bool, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM membership_payments WHERE athlete_id = ?1 AND month = ?2 AND status = 'Approved'",
            (athlete_id.to_string(), month.to_string()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let n: i64 = row.and_then(|r| r.get(0).ok()).unwrap_or(0);
    Ok(n > 0)
}

pub async fn create_my_payment(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<CreateMyPaymentRequest>,
) -> Result<StatusCode, ApiError> {
    let athlete_id = my_athlete_id(&state, &claims).await?;
    let month = payload
        .month
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(current_month_yyyy_mm);
    let _ = due_date_yyyy_mm_10(&month)?; // walidacja formatu

    if is_month_paid_approved(&state, &athlete_id, &month).await? {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Ta składka jest już oznaczona jako opłacona (zatwierdzona).",
        ));
    }

    // Nie spamuj wieloma pendingami dla tego samego miesiąca
    let mut existing = state
        .db
        .query(
            "SELECT COUNT(*) FROM membership_payments WHERE athlete_id = ?1 AND month = ?2 AND status = 'Pending'",
            (athlete_id.clone(), month.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let n_pending: i64 = existing
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get(0).ok())
        .unwrap_or(0);
    if n_pending > 0 {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Masz już zgłoszenie płatności (Pending) dla tego miesiąca.",
        ));
    }

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let note = payload.note.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    state
        .db
        .execute(
            "INSERT INTO membership_payments (id, athlete_id, month, amount_pln, note, status, created_by_user_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'Pending', ?6, ?7)",
            (
                id,
                athlete_id,
                month,
                payload.amount_pln,
                note,
                claims.sub,
                now,
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::CREATED)
}

pub async fn my_payment_status(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<MonthQuery>,
) -> Result<Json<PaymentStatusResponse>, ApiError> {
    let athlete_id = my_athlete_id(&state, &claims).await?;
    let month = q
        .month
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(current_month_yyyy_mm);
    let due = due_date_yyyy_mm_10(&month)?;

    let is_paid = is_month_paid_approved(&state, &athlete_id, &month).await?;

    let today = Utc::now().date_naive();
    let is_overdue = today >= due && today.day() >= 10 && !is_paid;

    Ok(Json(PaymentStatusResponse {
        month: month.clone(),
        due_date: due.format("%Y-%m-%d").to_string(),
        is_paid,
        is_overdue,
    }))
}

pub async fn list_athletes_payment_status(
    State(state): State<AppState>,
    _claims: Claims,
    Query(q): Query<MonthQuery>,
) -> Result<Json<Vec<AthletePaymentStatusRow>>, ApiError> {
    let month = q
        .month
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(current_month_yyyy_mm);
    let _ = due_date_yyyy_mm_10(&month)?;

    let mut rows = state
        .db
        .query(
            "SELECT a.id, a.full_name, \
                (SELECT COUNT(*) FROM membership_payments p WHERE p.athlete_id = a.id AND p.month = ?1 AND p.status = 'Approved') AS paid_count \
             FROM athletes a \
             WHERE (a.is_active IS NULL OR a.is_active = 1) \
             ORDER BY a.full_name ASC",
            [month.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out: Vec<AthletePaymentStatusRow> = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let athlete_id = sql_row::string(&r, 0).unwrap_or_default();
        let full_name = sql_row::string(&r, 1).unwrap_or_else(|_| "?".to_string());
        let paid_count: i64 = r.get(2).unwrap_or(0);
        out.push(AthletePaymentStatusRow {
            athlete_id,
            full_name,
            is_paid: paid_count > 0,
        });
    }

    Ok(Json(out))
}

pub async fn list_pending_payments(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<PendingPaymentRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT p.id, p.athlete_id, a.full_name, p.month, p.amount_pln, p.note, p.created_at, p.created_by_user_id \
             FROM membership_payments p \
             JOIN athletes a ON a.id = p.athlete_id \
             WHERE p.status = 'Pending' \
             ORDER BY p.created_at DESC",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(PendingPaymentRow {
            id: sql_row::string(&r, 0).unwrap_or_default(),
            athlete_id: sql_row::string(&r, 1).unwrap_or_default(),
            athlete_name: sql_row::string(&r, 2).unwrap_or_else(|_| "?".to_string()),
            month: sql_row::string(&r, 3).unwrap_or_default(),
            amount_pln: r.get(4).ok(),
            note: sql_row::opt_string(&r, 5).unwrap_or(None),
            created_at: sql_row::string(&r, 6).unwrap_or_default(),
            created_by_user_id: sql_row::opt_string(&r, 7).unwrap_or(None),
        });
    }
    Ok(Json(out))
}

async fn set_payment_status(
    state: &AppState,
    id: &str,
    new_status: &str,
    actor_user_id: &str,
) -> Result<u64, ApiError> {
    let now = Utc::now().to_rfc3339();
    let res = state
        .db
        .execute(
            "UPDATE membership_payments \
             SET status = ?1, approved_by_user_id = ?2, approved_at = ?3 \
             WHERE id = ?4 AND status = 'Pending'",
            (
                new_status.to_string(),
                actor_user_id.to_string(),
                now,
                id.to_string(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(res)
}

pub async fn approve_payment(
    State(state): State<AppState>,
    auth: RequireTrainerOrHigher,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let n = set_payment_status(&state, &id, "Approved", &auth.0.sub).await?;
    if n == 0 {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Zgłoszenie nie znalezione (lub nie jest już w statusie Pending).",
        ));
    }
    Ok(StatusCode::OK)
}

pub async fn reject_payment(
    State(state): State<AppState>,
    auth: RequireTrainerOrHigher,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let n = set_payment_status(&state, &id, "Rejected", &auth.0.sub).await?;
    if n == 0 {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Zgłoszenie nie znalezione (lub nie jest już w statusie Pending).",
        ));
    }
    Ok(StatusCode::OK)
}

#[derive(Debug, Deserialize)]
pub struct CreateApprovedPaymentRequest {
    pub month: Option<String>,
    pub amount_pln: Option<f64>,
    pub note: Option<String>,
}

pub async fn create_approved_payment_for_athlete(
    State(state): State<AppState>,
    auth: RequireTrainerOrHigher,
    Path(athlete_id): Path<String>,
    Json(payload): Json<CreateApprovedPaymentRequest>,
) -> Result<StatusCode, ApiError> {
    let month = payload
        .month
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(current_month_yyyy_mm);
    let _ = due_date_yyyy_mm_10(&month)?;

    if is_month_paid_approved(&state, &athlete_id, &month).await? {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Ta składka jest już zatwierdzona jako opłacona.",
        ));
    }

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let note = payload.note.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    state
        .db
        .execute(
            "INSERT INTO membership_payments (id, athlete_id, month, amount_pln, note, status, created_by_user_id, created_at, approved_by_user_id, approved_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'Approved', ?6, ?7, ?8, ?9)",
            (
                id,
                athlete_id,
                month,
                payload.amount_pln,
                note,
                auth.0.sub.clone(),
                now.clone(),
                auth.0.sub,
                now,
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::CREATED)
}

