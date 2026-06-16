use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, RequireTrainerOrHigher};
use crate::sql_row;
use crate::state::AppState;

const MONTHLY_FEE_PLN: f64 = 50.0;

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
    /// Z profilu zawodnika — UI może pokazać „przelew stały” zamiast mylącego „nieopłacona”, zanim auto-składka się zaksięguje.
    #[serde(default)]
    pub has_standing_order: bool,
}

#[derive(Debug, Deserialize)]
pub struct MonthQuery {
    pub month: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct YearQuery {
    pub year: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct AthletePaymentStatusRow {
    pub athlete_id: String,
    pub full_name: String,
    pub is_paid: bool,
}

#[derive(Debug, Serialize)]
pub struct PaymentMonthStatusRow {
    pub month: String,    // YYYY-MM
    pub due_date: String, // YYYY-MM-10
    pub is_paid: bool,
    pub has_pending: bool,
    pub is_overdue: bool,
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

#[derive(Debug, Serialize)]
pub struct AthletePaymentOverviewRow {
    pub athlete_id: String,
    pub full_name: String,
    pub month: String,
    pub has_approved: bool,
    pub has_pending: bool,
    pub approved_amount_pln: f64,
    pub pending_amount_pln: f64,
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

fn add_months_yyyy_mm(month: &str, delta: i32) -> Result<String, ApiError> {
    let (mut y, m) = parse_month_yyyy_mm(month)?;
    let mut mi = m as i32 + delta;
    while mi > 12 {
        mi -= 12;
        y += 1;
    }
    while mi < 1 {
        mi += 12;
        y -= 1;
    }
    Ok(format!("{:04}-{:02}", y, mi))
}

async fn my_athlete_id(state: &AppState, claims: &Claims) -> Result<String, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id FROM athletes WHERE user_id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            api_error(
                StatusCode::NOT_FOUND,
                "Nie znaleziono profilu zawodnika dla tego konta",
            )
        })?;
    let id: String = row.get(0).unwrap_or_default();
    if id.trim().is_empty() {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Nie znaleziono profilu zawodnika dla tego konta",
        ));
    }
    Ok(id)
}

async fn is_month_paid_approved(
    state: &AppState,
    athlete_id: &str,
    month: &str,
) -> Result<bool, ApiError> {
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

fn normalize_amount_pln(amount: Option<f64>) -> Result<f64, ApiError> {
    let a = amount.unwrap_or(MONTHLY_FEE_PLN);
    if !a.is_finite() || a <= 0.0 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Nieprawidłowa kwota płatności.",
        ));
    }
    Ok(a)
}

async fn insert_approved_payment_row(
    state: &AppState,
    athlete_id: &str,
    month: &str,
    amount_pln: f64,
    note: Option<String>,
    actor_user_id: &str,
    created_by_user_id: Option<String>,
) -> Result<(), ApiError> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    state
        .db
        .execute(
            "INSERT INTO membership_payments (id, athlete_id, month, amount_pln, note, status, created_by_user_id, created_at, approved_by_user_id, approved_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'Approved', ?6, ?7, ?8, ?9)",
            (
                id,
                athlete_id.to_string(),
                month.to_string(),
                Some(amount_pln),
                note,
                created_by_user_id,
                now.clone(),
                actor_user_id.to_string(),
                now,
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

/// Rozbija płatność na kolejne *nieopłacone* miesiące po 50 PLN.
async fn allocate_approved_payment_with_carry(
    state: &AppState,
    athlete_id: &str,
    start_month: &str,
    total_amount_pln: f64,
    note: Option<String>,
    actor_user_id: &str,
    created_by_user_id: Option<String>,
) -> Result<Vec<String>, ApiError> {
    let mut remaining = total_amount_pln;
    let mut cursor_month = start_month.to_string();
    let mut affected_months: Vec<String> = Vec::new();

    // Limit bezpieczeństwa: nawet duża kwota nie może wpaść w nieskończoną pętlę.
    for _ in 0..240 {
        if remaining <= 0.0 {
            break;
        }
        let _ = due_date_yyyy_mm_10(&cursor_month)?; // walidacja

        // Jeśli miesiąc już opłacony, przeskocz dalej (nadpłata idzie na kolejne nieopłacone).
        if is_month_paid_approved(state, athlete_id, &cursor_month).await? {
            cursor_month = add_months_yyyy_mm(&cursor_month, 1)?;
            continue;
        }

        let chunk = remaining.min(MONTHLY_FEE_PLN);
        insert_approved_payment_row(
            state,
            athlete_id,
            &cursor_month,
            chunk,
            note.clone(),
            actor_user_id,
            created_by_user_id.clone(),
        )
        .await?;
        affected_months.push(cursor_month.clone());
        remaining -= chunk;
        cursor_month = add_months_yyyy_mm(&cursor_month, 1)?;
    }

    Ok(affected_months)
}

pub async fn create_my_payment(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<CreateMyPaymentRequest>,
) -> Result<StatusCode, ApiError> {
    if let Err(()) =
        crate::post_throttle::record_user_post_attempt(&claims.sub, "payments_my_create")
    {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Zbyt wiele zgłoszeń składki w krótkim czasie. Odczekaj chwilę i spróbuj ponownie.",
        ));
    }

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
    let note = payload
        .note
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(a) = payload.amount_pln
        && (!a.is_finite() || a <= 0.0) {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Nieprawidłowa kwota płatności.",
            ));
        }

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

pub(crate) async fn payment_status_for_athlete_id(
    state: &AppState,
    athlete_id: &str,
    month: &str,
) -> Result<PaymentStatusResponse, ApiError> {
    athlete_exists(state, athlete_id).await?;
    let due = due_date_yyyy_mm_10(month)?;
    let is_paid = is_month_paid_approved(state, athlete_id, month).await?;

    let mut standing_rows = state
        .db
        .query(
            "SELECT COALESCE(has_standing_order, 0) FROM athletes WHERE id = ?1",
            [athlete_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let standing_row = standing_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let has_standing_order: bool =
        standing_row.and_then(|r| r.get::<i64>(0).ok()).unwrap_or(0) != 0;

    let today = Utc::now().date_naive();
    let is_overdue = today >= due && today.day() >= 10 && !is_paid;

    Ok(PaymentStatusResponse {
        month: month.to_string(),
        due_date: due.format("%Y-%m-%d").to_string(),
        is_paid,
        is_overdue,
        has_standing_order,
    })
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
    Ok(Json(
        payment_status_for_athlete_id(&state, &athlete_id, &month).await?,
    ))
}

async fn athlete_exists(state: &AppState, athlete_id: &str) -> Result<(), ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id FROM athletes WHERE id = ?1",
            [athlete_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"));
    }
    Ok(())
}

async fn year_status_for_athlete(
    state: &AppState,
    athlete_id: &str,
    year: i32,
) -> Result<Vec<PaymentMonthStatusRow>, ApiError> {
    let year_prefix = format!("{:04}-", year);

    // Jedno zapytanie do zbudowania mapy miesiąc -> (approved/pending).
    let mut rows = state
        .db
        .query(
            "SELECT month, \
                (SELECT COUNT(*) FROM membership_payments p2 WHERE p2.athlete_id = ?1 AND p2.month = p.month AND p2.status = 'Approved') AS approved_count, \
                (SELECT COUNT(*) FROM membership_payments p3 WHERE p3.athlete_id = ?1 AND p3.month = p.month AND p3.status = 'Pending')  AS pending_count \
             FROM membership_payments p \
             WHERE p.athlete_id = ?1 AND p.month LIKE ?2 \
             GROUP BY month",
            (athlete_id.to_string(), format!("{}%", year_prefix)),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use std::collections::HashMap;
    let mut map: HashMap<String, (i64, i64)> = HashMap::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let m: String = r.get(0).unwrap_or_default();
        let approved_count: i64 = r.get(1).unwrap_or(0);
        let pending_count: i64 = r.get(2).unwrap_or(0);
        if !m.trim().is_empty() {
            map.insert(m, (approved_count, pending_count));
        }
    }

    let today = Utc::now().date_naive();
    let mut out: Vec<PaymentMonthStatusRow> = Vec::with_capacity(12);
    for mm in 1..=12 {
        let month = format!("{:04}-{:02}", year, mm);
        let due = due_date_yyyy_mm_10(&month)?;
        let (approved_count, pending_count) = map.get(&month).copied().unwrap_or((0, 0));
        let is_paid = approved_count > 0;
        let has_pending = pending_count > 0;
        let is_overdue = today >= due && today.day() >= 10 && !is_paid;
        out.push(PaymentMonthStatusRow {
            month,
            due_date: due.format("%Y-%m-%d").to_string(),
            is_paid,
            has_pending,
            is_overdue,
        });
    }
    Ok(out)
}

pub async fn my_payments_year(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<YearQuery>,
) -> Result<Json<Vec<PaymentMonthStatusRow>>, ApiError> {
    let athlete_id = my_athlete_id(&state, &claims).await?;
    let year = q.year.unwrap_or_else(|| Utc::now().year());
    let rows = year_status_for_athlete(&state, &athlete_id, year).await?;
    Ok(Json(rows))
}

pub async fn athlete_payments_year(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
    Path(athlete_id): Path<String>,
    Query(q): Query<YearQuery>,
) -> Result<Json<Vec<PaymentMonthStatusRow>>, ApiError> {
    athlete_exists(&state, &athlete_id).await?;
    let year = q.year.unwrap_or_else(|| Utc::now().year());
    let rows = year_status_for_athlete(&state, &athlete_id, year).await?;
    Ok(Json(rows))
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
            crate::sql::queries::payments::ATHLETES_PAYMENT_STATUS_FOR_MONTH,
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

/// Widok kadry: dla danego miesiąca zwraca statusy: Approved / Pending / None (brak wpłaty).
pub async fn payments_overview_for_month(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
    Query(q): Query<MonthQuery>,
) -> Result<Json<Vec<AthletePaymentOverviewRow>>, ApiError> {
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
            crate::sql::queries::payments::PAYMENTS_OVERVIEW_FOR_MONTH,
            [month.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out: Vec<AthletePaymentOverviewRow> = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let athlete_id = sql_row::string(&r, 0).unwrap_or_default();
        let full_name = sql_row::string(&r, 1).unwrap_or_else(|_| "?".to_string());
        let approved_count: i64 = r.get(2).unwrap_or(0);
        let pending_count: i64 = r.get(3).unwrap_or(0);
        let approved_sum: f64 = sql_row::opt_f64(&r, 4).unwrap_or(None).unwrap_or(0.0);
        let pending_sum: f64 = sql_row::opt_f64(&r, 5).unwrap_or(None).unwrap_or(0.0);
        out.push(AthletePaymentOverviewRow {
            athlete_id,
            full_name,
            month: month.clone(),
            has_approved: approved_count > 0,
            has_pending: pending_count > 0,
            approved_amount_pln: approved_sum,
            pending_amount_pln: pending_sum,
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
            amount_pln: sql_row::opt_f64(&r, 4).unwrap_or(None),
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
    // Pobierz zgłoszenie (Pending) — potrzebujemy miesiąca i kwoty (do carryover).
    let mut rows = state
        .db
        .query(
            "SELECT athlete_id, month, amount_pln, note, created_by_user_id FROM membership_payments WHERE id = ?1 AND status = 'Pending'",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            api_error(
                StatusCode::NOT_FOUND,
                "Zgłoszenie nie znalezione (lub nie jest już w statusie Pending).",
            )
        })?;

    let athlete_id: String = row.get(0).unwrap_or_default();
    let month: String = row.get(1).unwrap_or_default();
    let amount_pln: Option<f64> = sql_row::opt_f64(&row, 2).unwrap_or(None);
    let note: Option<String> = sql_row::opt_string(&row, 3).unwrap_or(None);
    let created_by_user_id: Option<String> = sql_row::opt_string(&row, 4).unwrap_or(None);

    let total = normalize_amount_pln(amount_pln)?;

    // Transakcja: ustaw Approved na zgłoszeniu + dołóż carryover jako nowe wiersze Approved.
    state
        .db
        .execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Oznacz oryginalne zgłoszenie jako Approved i wstaw kwotę maks. 50 dla miesiąca zgłoszenia.
    let now = Utc::now().to_rfc3339();
    let first_chunk = total.min(MONTHLY_FEE_PLN);
    let updated = state
        .db
        .execute(
            "UPDATE membership_payments \
             SET status = 'Approved', amount_pln = ?1, approved_by_user_id = ?2, approved_at = ?3 \
             WHERE id = ?4 AND status = 'Pending'",
            (
                Some(first_chunk),
                auth.0.sub.clone(),
                now.clone(),
                id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if updated == 0 {
        let _ = state.db.execute("ROLLBACK", ()).await;
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Zgłoszenie nie znalezione (lub nie jest już w statusie Pending).",
        ));
    }

    // Reszta kwoty idzie na kolejne miesiące (pomijając już opłacone).
    let mut remaining = total - first_chunk;
    let mut cursor_month = add_months_yyyy_mm(&month, 1)?;
    for _ in 0..240 {
        if remaining <= 0.0 {
            break;
        }
        if is_month_paid_approved(&state, &athlete_id, &cursor_month).await? {
            cursor_month = add_months_yyyy_mm(&cursor_month, 1)?;
            continue;
        }
        let chunk = remaining.min(MONTHLY_FEE_PLN);
        insert_approved_payment_row(
            &state,
            &athlete_id,
            &cursor_month,
            chunk,
            note.clone(),
            &auth.0.sub,
            created_by_user_id.clone(),
        )
        .await?;
        remaining -= chunk;
        cursor_month = add_months_yyyy_mm(&cursor_month, 1)?;
    }

    state
        .db
        .execute("COMMIT", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

#[derive(Debug, Deserialize)]
pub struct StandingOrderRequest {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct StandingOrderResponse {
    pub athlete_id: String,
    pub has_standing_order: bool,
}

/// Włącza/wyłącza zlecenie stałe na składkę dla zawodnika.
///
/// Gdy włączono, scheduler automatycznie zaznaczy bieżący miesiąc jako opłacony
/// (Approved-payment, kwota = `MONTHLY_FEE_PLN`) o ile nie ma już Approved-wpisu.
/// Jeśli scheduler znajdzie tę flagę przy włączeniu (np. kilka dni po starcie miesiąca),
/// utworzy płatność „wstecznie" za bieżący miesiąc.
pub async fn set_standing_order(
    State(state): State<AppState>,
    auth: RequireTrainerOrHigher,
    Path(athlete_id): Path<String>,
    Json(payload): Json<StandingOrderRequest>,
) -> Result<Json<StandingOrderResponse>, ApiError> {
    let value: i64 = if payload.enabled { 1 } else { 0 };
    let updated = state
        .db
        .execute(
            "UPDATE athletes SET has_standing_order = ?1 WHERE id = ?2",
            (value, athlete_id.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if updated == 0 {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Nie znaleziono zawodnika.",
        ));
    }

    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&auth.0.sub),
        Some("trainer"),
        "payments",
        if payload.enabled {
            "standing_order_enabled"
        } else {
            "standing_order_disabled"
        },
        Some("athlete"),
        Some(&athlete_id),
        Some(
            &serde_json::json!({ "athlete_id": athlete_id, "enabled": payload.enabled })
                .to_string(),
        ),
    )
    .await;

    // Przy włączaniu od razu próbujemy „dogonić" bieżący miesiąc — jeśli zawodnik nie miał
    // jeszcze Approved-wpisu, tworzymy go teraz, żeby nie czekać do nocnego cyklu.
    if payload.enabled {
        let current_month = current_month_yyyy_mm();
        if !is_month_paid_approved(&state, &athlete_id, &current_month).await? {
            let now = Utc::now().to_rfc3339();
            let id = Uuid::new_v4().to_string();
            let _ = state
                .db
                .execute(
                    "INSERT INTO membership_payments (id, athlete_id, month, amount_pln, note, status, created_by_user_id, created_at, approved_by_user_id, approved_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, 'Approved', ?6, ?7, ?8, ?9)",
                    (
                        id,
                        athlete_id.clone(),
                        current_month.clone(),
                        Some(MONTHLY_FEE_PLN),
                        Some("Przelew stały (auto)".to_string()),
                        auth.0.sub.clone(),
                        now.clone(),
                        auth.0.sub.clone(),
                        now,
                    ),
                )
                .await;
        }
    }

    Ok(Json(StandingOrderResponse {
        athlete_id,
        has_standing_order: payload.enabled,
    }))
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

    let total = normalize_amount_pln(payload.amount_pln)?;
    let note = payload
        .note
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Rozbij na miesiące po 50 (pomijaj już opłacone).
    let months = allocate_approved_payment_with_carry(
        &state,
        &athlete_id,
        &month,
        total,
        note,
        &auth.0.sub,
        Some(auth.0.sub.clone()),
    )
    .await?;

    if months.is_empty() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Składka jest już opłacona dla bieżących miesięcy (brak miejsca na alokację nadpłaty).",
        ));
    }

    Ok(StatusCode::CREATED)
}
