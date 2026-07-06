use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use libsql::Row;

use crate::api_error::{ApiError, api_error};
use crate::db;
use crate::pagination::{ListPaginationQuery, parse_list_pagination};
use crate::middleware::auth::{
    Claims, RequireTrainerOrHigher, claims_has_staff_access, claims_is_pure_athlete,
};
use crate::models::{CompetitionResult, PublicResultBoardRow, ResultKind, ResultStatus, Role};
use crate::sql_row;
use crate::state::AppState;

/// Domyślne „miejsce" dla wyników treningowych — wszystkie treningi odbywają się na sali klubowej.
const TRAINING_DEFAULT_LOCATION: &str = "Slavia";

/// Filtr `?kind=competition|training|all` dla list wyników. Domyślnie tylko zawody (publiczne).
#[derive(Debug, Deserialize, Default)]
pub struct ResultsListQuery {
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KindFilter {
    Competition,
    Training,
    All,
}

fn parse_kind_filter(raw: Option<&str>) -> Result<KindFilter, ApiError> {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("competition") | Some("comp") => Ok(KindFilter::Competition),
        Some("training") | Some("train") => Ok(KindFilter::Training),
        Some("all") | Some("any") | Some("*") => Ok(KindFilter::All),
        Some(other) => Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("Invalid kind filter: {other}"),
        )),
    }
}

pub(crate) async fn sync_athlete_bests_from_approved(
    state: &AppState,
    athlete_id: &str,
) -> Result<(), ApiError> {
    let conn_arc = state.db.raw().await;
    db::sync_athlete_bests_from_approved_conn(conn_arc.as_ref(), athlete_id)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

pub(crate) async fn sync_athletes_bests_from_approved_batch(
    state: &AppState,
    athlete_ids: &[String],
) -> Result<(), ApiError> {
    if athlete_ids.is_empty() {
        return Ok(());
    }
    let conn_arc = state.db.raw().await;
    db::sync_athletes_bests_from_approved_batch_conn(conn_arc.as_ref(), athlete_ids)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn result_row_athlete_id(state: &AppState, result_id: &str) -> Result<String, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT athlete_id FROM results WHERE id = ?1",
            [result_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Result not found"))?;

    sql_row::required_string(&row, 0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))
}

async fn ensure_can_view_athlete_submissions(
    state: &AppState,
    claims: &Claims,
    athlete_id: &str,
) -> Result<(), ApiError> {
    if claims_has_staff_access(claims) {
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
        return Ok(());
    }
    if claims.roles.contains(&Role::Athlete) {
        let mut rows = state
            .db
            .query(
                "SELECT id FROM athletes WHERE id = ?1 AND user_id = ?2",
                (athlete_id.to_string(), claims.sub.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_none()
        {
            return Err(api_error(
                StatusCode::FORBIDDEN,
                "You can only view your own submissions",
            ));
        }
        return Ok(());
    }
    Err(api_error(
        StatusCode::FORBIDDEN,
        "Insufficient permissions to view submissions",
    ))
}

/// Wiersz oczekuje kolumn:
/// `id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg, kind, location, bodyweight_kg`
pub(crate) fn competition_result_from_row(row: &Row) -> Result<CompetitionResult, String> {
    let status_str = sql_row::required_string(row, 5)?;
    let status = status_str
        .parse::<ResultStatus>()
        .map_err(|e| format!("{} (status={:?})", e, status_str))?;
    let kind_str = sql_row::opt_string(row, 10)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| "competition".to_string());
    let kind = kind_str
        .parse::<ResultKind>()
        .unwrap_or(ResultKind::Competition);
    Ok(CompetitionResult {
        id: sql_row::required_string(row, 0)?,
        athlete_id: sql_row::required_string(row, 1)?,
        snatch: sql_row::required_f64(row, 2)?,
        clean_and_jerk: sql_row::required_f64(row, 3)?,
        total: sql_row::required_f64(row, 4)?,
        status,
        date: sql_row::required_string(row, 6)?,
        kind,
        location: sql_row::opt_string(row, 11).map_err(|e| e.to_string())?,
        bodyweight_kg: sql_row::opt_f64(row, 12).map_err(|e| e.to_string())?,
        squat_kg: sql_row::opt_f64(row, 7).map_err(|e| e.to_string())?,
        bench_kg: sql_row::opt_f64(row, 8).map_err(|e| e.to_string())?,
        deadlift_kg: sql_row::opt_f64(row, 9).map_err(|e| e.to_string())?,
    })
}

#[derive(Deserialize)]
pub struct CreateResultRequest {
    pub athlete_id: String,
    /// Brak pola — przy tworzeniu wpisu użyj aktualnych wartości z profilu zawodnika (`athletes.best_*`).
    #[serde(default)]
    pub snatch: Option<f64>,
    #[serde(default)]
    pub clean_and_jerk: Option<f64>,
    #[serde(default)]
    pub total: Option<f64>,
    pub date: String,
    #[serde(default)]
    pub squat_kg: Option<f64>,
    #[serde(default)]
    pub bench_kg: Option<f64>,
    #[serde(default)]
    pub deadlift_kg: Option<f64>,
    /// `competition` (domyślnie) albo `training` — trening jest niepubliczny i nie wpływa na PB w `athletes`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Miejsce zawodów — wypełniane tylko dla `kind = 'competition'`.
    #[serde(default)]
    pub location: Option<String>,
    /// Waga ciała na starcie (kg) — opcjonalna; do Sinclaira i statystyk.
    #[serde(default)]
    pub bodyweight_kg: Option<f64>,
}

async fn athlete_oly_baseline(
    state: &AppState,
    athlete_id: &str,
) -> Result<(f64, f64, f64), ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT best_snatch_kg, best_clean_jerk_kg, total_kg FROM athletes WHERE id = ?1",
            [athlete_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete not found"))?;

    let sn_o: Option<f64> = row.get(0).ok();
    let cj_o: Option<f64> = row.get(1).ok();
    let tot_o: Option<f64> = row.get(2).ok();
    let sn = sn_o.unwrap_or(0.0);
    let cj = cj_o.unwrap_or(0.0);
    let tot = tot_o.unwrap_or(sn + cj);
    Ok((sn, cj, tot))
}

/// Publiczna tablica wyników z imieniem zawodnika i miejscem zawodów (tylko odczyt; wyłącznie `kind = competition`).
pub async fn list_public_results_board(
    State(state): State<AppState>,
) -> Result<Json<Vec<PublicResultBoardRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT r.id, r.athlete_id, a.full_name, r.competition_id, c.title, \
             r.snatch, r.clean_and_jerk, r.total, r.date, r.squat_kg, r.bench_kg, r.deadlift_kg, r.location \
             FROM results r \
             INNER JOIN athletes a ON a.id = r.athlete_id \
             LEFT JOIN competitions c ON c.id = r.competition_id \
             WHERE r.status = 'Approved' AND r.kind = 'competition' \
             ORDER BY r.date DESC, r.total DESC \
             LIMIT 500",
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
        let competition_title = sql_row::opt_string(&row, 4)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        out.push(PublicResultBoardRow {
            id: sql_row::required_string(&row, 0)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_id: sql_row::required_string(&row, 1)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_name: sql_row::required_string(&row, 2)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            competition_id: sql_row::opt_string(&row, 3)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            competition_title,
            snatch: sql_row::required_f64(&row, 5)
                .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            clean_and_jerk: sql_row::required_f64(&row, 6)
                .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            total: sql_row::required_f64(&row, 7)
                .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            date: sql_row::required_string(&row, 8)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            kind: ResultKind::Competition,
            location: sql_row::opt_string(&row, 12)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            squat_kg: sql_row::opt_f64(&row, 9)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            bench_kg: sql_row::opt_f64(&row, 10)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            deadlift_kg: sql_row::opt_f64(&row, 11)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        });
    }

    Ok(Json(out))
}

/// Publiczna tablica klasycznego dwuboju (bez wpisów stricte siłowych; wyłącznie `kind = competition`).
pub async fn list_public_olympic_board(
    State(state): State<AppState>,
) -> Result<Json<Vec<PublicResultBoardRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT r.id, r.athlete_id, a.full_name, r.competition_id, c.title, \
             r.snatch, r.clean_and_jerk, r.total, r.date, r.squat_kg, r.bench_kg, r.deadlift_kg, r.location \
             FROM results r \
             INNER JOIN athletes a ON a.id = r.athlete_id \
             LEFT JOIN competitions c ON c.id = r.competition_id \
             WHERE r.status = 'Approved' AND r.kind = 'competition' AND r.total >= 20 \
             ORDER BY r.date DESC, r.total DESC",
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
        out.push(PublicResultBoardRow {
            id: sql_row::required_string(&row, 0)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_id: sql_row::required_string(&row, 1)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_name: sql_row::required_string(&row, 2)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            competition_id: sql_row::opt_string(&row, 3)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            competition_title: sql_row::opt_string(&row, 4)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            snatch: sql_row::required_f64(&row, 5)
                .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            clean_and_jerk: sql_row::required_f64(&row, 6)
                .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            total: sql_row::required_f64(&row, 7)
                .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            date: sql_row::required_string(&row, 8)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            kind: ResultKind::Competition,
            location: sql_row::opt_string(&row, 12)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            squat_kg: sql_row::opt_f64(&row, 9)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            bench_kg: sql_row::opt_f64(&row, 10)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            deadlift_kg: sql_row::opt_f64(&row, 11)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        });
    }
    Ok(Json(out))
}

pub async fn list_approved_results(
    State(state): State<AppState>,
    Query(pagination): Query<ListPaginationQuery>,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let (limit, offset) = parse_list_pagination(&pagination, 500, 2000);
    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg, kind, location, bodyweight_kg \
             FROM results WHERE status = 'Approved' AND kind = 'competition' \
             ORDER BY date DESC LIMIT ?1 OFFSET ?2",
            (limit, offset),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

/// Wszystkie zgłoszenia w statusie Pending — wspólne dla listy kadry i bundle dashboardu trenera.
pub(crate) async fn fetch_all_pending_results(
    state: &AppState,
) -> Result<Vec<CompetitionResult>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg, kind, location, bodyweight_kg \
             FROM results WHERE status = 'Pending' ORDER BY date DESC, id DESC",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(results)
}

pub async fn list_pending_results(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    Ok(Json(fetch_all_pending_results(&state).await?))
}

/// Publiczne karty / ranking / wykresy.
/// Domyślnie tylko `kind = 'competition'` (publiczne, bez auth).
/// Dla `?kind=training` lub `?kind=all` wymaga sesji oraz uprawnień jak przy podglądzie zgłoszeń zawodnika:
/// kadra lub zalogowany zawodnik powiązany z tym rekordem `athlete_id` (niepubliczny trening nie „wycieknie” przy skanowaniu cudzych profili).
pub async fn list_athlete_results(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    Query(q): Query<ResultsListQuery>,
    claims: Option<Claims>,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let kind_filter = parse_kind_filter(q.kind.as_deref())?;
    if matches!(kind_filter, KindFilter::Training | KindFilter::All) {
        let Some(c) = claims.as_ref() else {
            return Err(api_error(
                StatusCode::UNAUTHORIZED,
                "Dane treningowe wymagają zalogowanego konta",
            ));
        };
        ensure_can_view_athlete_submissions(&state, c, &athlete_id).await?;
    }

    let kind_clause = match kind_filter {
        KindFilter::Competition => " AND r.kind = 'competition'",
        KindFilter::Training => " AND r.kind = 'training'",
        KindFilter::All => "",
    };

    let sql = format!(
        "SELECT r.id, r.athlete_id, r.snatch, r.clean_and_jerk, r.total, r.status, r.date, r.squat_kg, r.bench_kg, r.deadlift_kg, r.kind, r.location, r.bodyweight_kg \
         FROM results r \
         INNER JOIN athletes a ON a.id = r.athlete_id AND (a.is_active IS NULL OR a.is_active = 1) \
         WHERE r.athlete_id = ?1 AND r.status = 'Approved'{kind_clause} ORDER BY r.date ASC"
    );

    let mut rows = state
        .db
        .query(&sql, [athlete_id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

/// Zawodnik (własny profil) lub kadra — wszystkie zgłoszenia (Pending + Approved).
/// Domyślnie zwraca oba rodzaje wpisów (`competition` + `training`); można zawęzić przez `?kind=`.
pub async fn list_athlete_result_submissions(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    Query(q): Query<ResultsListQuery>,
    claims: Claims,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    ensure_can_view_athlete_submissions(&state, &claims, &athlete_id).await?;

    // Domyślnie `All`, bo zawodnik widzi własne wpisy obu typów.
    let kind_filter = match q.kind.as_deref() {
        None | Some("") => KindFilter::All,
        Some(other) => parse_kind_filter(Some(other))?,
    };
    let kind_clause = match kind_filter {
        KindFilter::Competition => " AND kind = 'competition'",
        KindFilter::Training => " AND kind = 'training'",
        KindFilter::All => "",
    };

    let sql = format!(
        "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg, kind, location, bodyweight_kg FROM results \
         WHERE athlete_id = ?1{kind_clause} ORDER BY date DESC, id DESC"
    );

    let mut rows = state
        .db
        .query(&sql, [athlete_id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

/// Liczba zgłoszeń w statusie Pending — używana m.in. w agregowanym dashboardzie zawodnika.
pub(crate) async fn pending_results_count_for_athlete_id(
    state: &AppState,
    athlete_id: &str,
) -> Result<i64, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT COUNT(*) FROM results WHERE athlete_id = ?1 AND status = 'Pending'",
            [athlete_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(row.and_then(|r| r.get::<i64>(0).ok()).unwrap_or(0))
}

pub async fn create_result(
    State(state): State<AppState>,
    claims: Claims, // Must be authenticated
    Json(payload): Json<CreateResultRequest>,
) -> Result<Json<CompetitionResult>, ApiError> {
    if let Err(()) = crate::post_throttle::record_user_post_attempt(&claims.sub, "results_create") {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Zbyt wiele zgłoszeń wyników w krótkim czasie. Odczekaj chwilę i spróbuj ponownie.",
        ));
    }

    let status = if claims_has_staff_access(&claims) {
        ResultStatus::Approved
    } else if claims.roles.contains(&Role::Athlete) {
        ResultStatus::Pending
    } else {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Insufficient role to create results",
        ));
    };

    if claims_is_pure_athlete(&claims) {
        let mut rows = state
            .db
            .query("SELECT id FROM athletes WHERE user_id = ?1", [claims.sub.clone()])
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            let athlete_id = sql_row::required_string(&row, 0)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            if athlete_id != payload.athlete_id {
                return Err(api_error(
                    StatusCode::FORBIDDEN,
                    "Athletes can only submit their own results",
                ));
            }
        } else {
            return Err(api_error(
                StatusCode::NOT_FOUND,
                "Athlete profile not found",
            ));
        }
    }

    let raw_sent_oly =
        payload.snatch.is_some() || payload.clean_and_jerk.is_some() || payload.total.is_some();
    let raw_sent_sbd = payload.squat_kg.map(|x| x > 0.0).unwrap_or(false)
        || payload.bench_kg.map(|x| x > 0.0).unwrap_or(false)
        || payload.deadlift_kg.map(|x| x > 0.0).unwrap_or(false);

    if !raw_sent_oly && !raw_sent_sbd {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Podaj przynajmniej rwanie lub podrzut albo jedno z ćwiczeń siłowych",
        ));
    }

    let (snatch, clean_and_jerk, total) = if raw_sent_oly {
        let baseline = athlete_oly_baseline(&state, &payload.athlete_id).await?;
        let snatch = payload.snatch.unwrap_or(baseline.0);
        let clean_and_jerk = payload.clean_and_jerk.unwrap_or(baseline.1);
        let total = payload.total.unwrap_or(snatch + clean_and_jerk);
        (snatch, clean_and_jerk, total)
    } else {
        (0.0, 0.0, 0.0)
    };

    if snatch < 0.0 || clean_and_jerk < 0.0 || total < 0.0 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Ciężary nie mogą być ujemne",
        ));
    }
    let oly_positive = snatch > 0.0 || clean_and_jerk > 0.0;
    if !oly_positive && !raw_sent_sbd {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Podaj dodatnie rwanie i/lub podrzut (0 dozwolone przy kontuzji/jednoboju) albo przynajmniej jedno ćwiczenie siłowe",
        ));
    }

    if claims_is_pure_athlete(&claims) {
        let mut dup_rows = state
            .db
            .query(
                "SELECT id FROM results WHERE athlete_id = ?1 AND date = ?2 AND total = ?3 AND status = 'Pending' LIMIT 1",
                (
                    payload.athlete_id.clone(),
                    payload.date.clone(),
                    total,
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if dup_rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_some()
        {
            return Err(api_error(
                StatusCode::CONFLICT,
                "Masz już zgłoszenie wyniku oczekujące na weryfikację z tym samym totalem i datą.",
            ));
        }
    }

    let id = Uuid::new_v4().to_string();
    let squat_kg = payload.squat_kg.filter(|x| *x > 0.0);
    let bench_kg = payload.bench_kg.filter(|x| *x > 0.0);
    let deadlift_kg = payload.deadlift_kg.filter(|x| *x > 0.0);
    let bodyweight_kg = payload.bodyweight_kg.filter(|x| *x > 0.0);

    let strength_only_notify = !raw_sent_oly && raw_sent_sbd;

    let kind = payload
        .kind
        .as_deref()
        .map(|s| s.parse::<ResultKind>())
        .transpose()
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?
        .unwrap_or(ResultKind::Competition);
    // Trening to zawsze sala klubowa — twardo wstawiamy "Slavia", ignorując ewentualne pole.
    // Dzięki temu w widokach wyników treningowych zawsze widać sensowne źródło wpisu.
    let location = if matches!(kind, ResultKind::Competition) {
        payload
            .location
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    } else {
        Some(TRAINING_DEFAULT_LOCATION.to_string())
    };

    state.db.execute(
        "INSERT INTO results (id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg, kind, location, bodyweight_kg) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        (
            id.clone(),
            payload.athlete_id.clone(),
            snatch,
            clean_and_jerk,
            total,
            status.to_string(),
            payload.date.clone(),
            squat_kg,
            bench_kg,
            deadlift_kg,
            kind.to_string(),
            location.clone(),
            bodyweight_kg,
        ),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    sync_athlete_bests_from_approved(&state, &payload.athlete_id).await?;

    if status == ResultStatus::Pending {
        let conn_arc = state.db.raw().await;
        let name = crate::notifications::athlete_full_name(conn_arc.as_ref(), &payload.athlete_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "Zawodnik".to_string());
        crate::notifications::notify_result_pending(
            &state,
            &payload.athlete_id,
            &name,
            total,
            &payload.date,
            strength_only_notify,
        );
    }

    Ok(Json(CompetitionResult {
        id,
        athlete_id: payload.athlete_id,
        snatch,
        clean_and_jerk,
        total,
        status,
        date: payload.date,
        kind,
        location,
        bodyweight_kg,
        squat_kg,
        bench_kg,
        deadlift_kg,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ReviewResultRequest {
    #[serde(default)]
    pub review_note: Option<String>,
}

pub async fn approve_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
    payload: Option<axum::Json<ReviewResultRequest>>,
) -> Result<StatusCode, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT athlete_id, total, date FROM results WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Result not found"))?;
    let athlete_id: String = row
        .get(0)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let total: f64 = row
        .get(1)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let date: String = row
        .get(2)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let n = state
        .db
        .execute(
            "UPDATE results SET status = 'Approved' WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Result not found"));
    }
    sync_athlete_bests_from_approved(&state, &athlete_id).await?;
    let note = payload.and_then(|p| p.0.review_note.filter(|s| !s.trim().is_empty()));
    crate::notifications::notify_result_approved(&state, &athlete_id, total, &date, note.as_deref());
    Ok(StatusCode::OK)
}

pub async fn reject_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
    payload: Option<axum::Json<ReviewResultRequest>>,
) -> Result<StatusCode, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT athlete_id, total, date FROM results WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Result not found"))?;
    let athlete_id: String = row
        .get(0)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let total: f64 = row
        .get(1)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let date: String = row
        .get(2)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let n = state
        .db
        .execute(
            "UPDATE results SET status = 'Rejected' WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Result not found"));
    }
    
    let note = payload.and_then(|p| p.0.review_note.filter(|s| !s.trim().is_empty()));
    crate::notifications::notify_result_rejected(&state, &athlete_id, total, &date, note.as_deref());
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub struct UpdateResultRequest {
    pub snatch: Option<f64>,
    pub clean_and_jerk: Option<f64>,
    pub total: Option<f64>,
    pub date: Option<String>,
    pub status: Option<String>,
    /// Brak pola — bez zmiany; `null` — wyczyść w bazie; liczba — ustaw.
    #[serde(default)]
    pub squat_kg: Option<Option<f64>>,
    #[serde(default)]
    pub bench_kg: Option<Option<f64>>,
    #[serde(default)]
    pub deadlift_kg: Option<Option<f64>>,
    /// `competition` lub `training`. Pomiń — bez zmiany.
    #[serde(default)]
    pub kind: Option<String>,
    /// Brak pola — bez zmiany; `null`/pusty string — wyczyść; tekst — ustaw.
    /// Ignorowane dla wpisów typu `training`.
    #[serde(default)]
    pub location: Option<Option<String>>,
    /// Brak pola — bez zmiany; `null` — wyczyść w bazie; liczba — ustaw.
    #[serde(default)]
    pub bodyweight_kg: Option<Option<f64>>,
}

pub async fn list_all_results_staff(
    State(state): State<AppState>,
    Query(q): Query<ResultsListQuery>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let kind_filter = match q.kind.as_deref() {
        None | Some("") => KindFilter::All,
        Some(other) => parse_kind_filter(Some(other))?,
    };
    let kind_clause = match kind_filter {
        KindFilter::Competition => " WHERE kind = 'competition'",
        KindFilter::Training => " WHERE kind = 'training'",
        KindFilter::All => "",
    };
    let sql = format!(
        "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg, kind, location, bodyweight_kg FROM results{kind_clause} ORDER BY date DESC"
    );
    let mut rows = state
        .db
        .query(&sql, ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

pub async fn update_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
    Json(payload): Json<UpdateResultRequest>,
) -> Result<Json<CompetitionResult>, ApiError> {
    if payload.snatch.is_none()
        && payload.clean_and_jerk.is_none()
        && payload.total.is_none()
        && payload.date.is_none()
        && payload.status.is_none()
        && payload.squat_kg.is_none()
        && payload.bench_kg.is_none()
        && payload.deadlift_kg.is_none()
        && payload.kind.is_none()
        && payload.location.is_none()
        && payload.bodyweight_kg.is_none()
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "At least one field is required",
        ));
    }

    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg, kind, location, bodyweight_kg FROM results WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Result not found"))?;

    let mut cr = competition_result_from_row(&row)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if let Some(v) = payload.snatch {
        if v < 0.0 {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Rwanie nie może być ujemne",
            ));
        }
        cr.snatch = v;
    }
    if let Some(v) = payload.clean_and_jerk {
        if v < 0.0 {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Podrzut nie może być ujemny",
            ));
        }
        cr.clean_and_jerk = v;
    }
    if let Some(v) = payload.total {
        if v < 0.0 {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Suma nie może być ujemna",
            ));
        }
        cr.total = v;
    } else if payload.snatch.is_some() || payload.clean_and_jerk.is_some() {
        cr.total = cr.snatch + cr.clean_and_jerk;
    }
    if let Some(d) = payload.date {
        cr.date = d;
    }
    if let Some(ref st) = payload.status {
        cr.status = st
            .parse::<ResultStatus>()
            .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    }
    if let Some(inner) = payload.squat_kg {
        cr.squat_kg = inner;
    }
    if let Some(inner) = payload.bench_kg {
        cr.bench_kg = inner;
    }
    if let Some(inner) = payload.deadlift_kg {
        cr.deadlift_kg = inner;
    }
    if let Some(ref k) = payload.kind {
        cr.kind = k
            .parse::<ResultKind>()
            .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    }
    if let Some(loc) = payload.location {
        cr.location = loc
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
    }
    if let Some(inner) = payload.bodyweight_kg {
        cr.bodyweight_kg = inner.filter(|x| *x > 0.0);
    }
    // Trening jest zawsze oznaczany jako sala klubowa „Slavia" — niezależnie od inputu.
    if matches!(cr.kind, ResultKind::Training) {
        cr.location = Some(TRAINING_DEFAULT_LOCATION.to_string());
    }

    state
        .db
        .execute(
            "UPDATE results SET snatch = ?1, clean_and_jerk = ?2, total = ?3, status = ?4, date = ?5, squat_kg = ?6, bench_kg = ?7, deadlift_kg = ?8, kind = ?9, location = ?10, bodyweight_kg = ?11 WHERE id = ?12",
            (
                cr.snatch,
                cr.clean_and_jerk,
                cr.total,
                cr.status.to_string(),
                cr.date.clone(),
                cr.squat_kg,
                cr.bench_kg,
                cr.deadlift_kg,
                cr.kind.to_string(),
                cr.location.clone(),
                cr.bodyweight_kg,
                id,
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let athlete_id = cr.athlete_id.clone();
    sync_athlete_bests_from_approved(&state, &athlete_id).await?;

    Ok(Json(cr))
}

pub async fn delete_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
) -> Result<StatusCode, ApiError> {
    let athlete_id = result_row_athlete_id(&state, &id).await?;

    let n = state
        .db
        .execute("DELETE FROM results WHERE id = ?1", [id])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Result not found"));
    }

    sync_athlete_bests_from_approved(&state, &athlete_id).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct BatchApproveRequest {
    pub ids: Vec<String>,
}

#[derive(Serialize)]
pub struct BatchApproveResponse {
    pub approved: u64,
    pub skipped: u64,
}

/// Zatwierdza wiele wyników oczekujących (`Pending`) jednym żądaniem — synchronizacja PB per zawodnik na końcu.
pub async fn batch_approve_results(
    State(state): State<AppState>,
    RequireTrainerOrHigher(claims): RequireTrainerOrHigher,
    Json(body): Json<BatchApproveRequest>,
) -> Result<Json<BatchApproveResponse>, ApiError> {
    if let Err(()) =
        crate::post_throttle::record_user_post_attempt(&claims.sub, "results_batch_approve")
    {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Zbyt wiele masowych akceptacji w krótkim czasie. Odczekaj chwilę i spróbuj ponownie.",
        ));
    }

    let ids: Vec<String> = body
        .ids
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(100)
        .collect();
    if ids.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Podaj co najmniej jeden identyfikator wyniku (ids).",
        ));
    }

    let conn = state.db.raw().await;
    let placeholders = crate::sql_util::in_placeholders(ids.len());
    let update_sql = format!(
        "UPDATE results SET status = 'Approved' WHERE status = 'Pending' AND id IN ({placeholders})"
    );
    let approved = conn
        .execute(&update_sql, ids.clone())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let skipped = ids.len() as u64 - approved;

    let select_sql = format!(
        "{}({placeholders})",
        crate::sql::queries::results::BATCH_APPROVE_SELECT_APPROVED
    );
    let mut rows = conn
        .query(&select_sql, ids.clone())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut athlete_ids = std::collections::HashSet::<String>::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let athlete_id: String = row
            .get(0)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let total: f64 = row
            .get(1)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let date: String = row
            .get(2)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        athlete_ids.insert(athlete_id.clone());
        crate::notifications::notify_result_approved(&state, &athlete_id, total, &date, None);
    }

    let athlete_list: Vec<String> = athlete_ids.into_iter().collect();
    sync_athletes_bests_from_approved_batch(&state, &athlete_list).await?;

    let details = serde_json::json!({
        "count": approved,
        "skipped": skipped,
        "sample_ids": ids.iter().take(12).collect::<Vec<_>>()
    })
    .to_string();
    let _ = crate::audit::write_audit_log(
        conn.as_ref(),
        Some(claims.sub.as_str()),
        Some("Trainer"),
        "results",
        "batch_approve",
        Some("results"),
        None,
        Some(details.as_str()),
    )
    .await;

    Ok(Json(BatchApproveResponse { approved, skipped }))
}
