use axum::{
    extract::{State, Path},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use libsql::Row;

use crate::api_error::{api_error, ApiError};
use crate::db;
use crate::state::AppState;
use crate::models::{CompetitionResult, PublicResultBoardRow, ResultStatus, Role};
use crate::middleware::auth::{claims_has_staff_access, claims_is_pure_athlete, Claims, RequireTrainerOrHigher};
use crate::sql_row;

pub(crate) async fn sync_athlete_bests_from_approved(
    state: &AppState,
    athlete_id: &str,
) -> Result<(), ApiError> {
    db::sync_athlete_bests_from_approved_conn(state.db.as_ref(), athlete_id)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn result_row_athlete_id(state: &AppState, result_id: &str) -> Result<String, ApiError> {
    let mut rows = state
        .db
        .query("SELECT athlete_id FROM results WHERE id = ?1", [result_id.to_string()])
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
            .query("SELECT id FROM athletes WHERE id = ?1", [athlete_id.to_string()])
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

fn competition_result_from_row(row: &Row) -> Result<CompetitionResult, String> {
    let status_str = sql_row::required_string(row, 5)?;
    let status = status_str
        .parse::<ResultStatus>()
        .map_err(|e| format!("{} (status={:?})", e, status_str))?;
    Ok(CompetitionResult {
        id: sql_row::required_string(row, 0)?,
        athlete_id: sql_row::required_string(row, 1)?,
        snatch: sql_row::required_f64(row, 2)?,
        clean_and_jerk: sql_row::required_f64(row, 3)?,
        total: sql_row::required_f64(row, 4)?,
        status,
        date: sql_row::required_string(row, 6)?,
        squat_kg: sql_row::opt_f64(row, 7).map_err(|e| e.to_string())?,
        bench_kg: sql_row::opt_f64(row, 8).map_err(|e| e.to_string())?,
        deadlift_kg: sql_row::opt_f64(row, 9).map_err(|e| e.to_string())?,
    })
}

#[derive(Deserialize)]
pub struct CreateResultRequest {
    pub athlete_id: String,
    pub snatch: f64,
    pub clean_and_jerk: f64,
    pub total: f64,
    pub date: String,
    #[serde(default)]
    pub squat_kg: Option<f64>,
    #[serde(default)]
    pub bench_kg: Option<f64>,
    #[serde(default)]
    pub deadlift_kg: Option<f64>,
}

/// Publiczna tablica wyników z imieniem zawodnika i nazwą zawodów (tylko odczyt).
pub async fn list_public_results_board(
    State(state): State<AppState>,
) -> Result<Json<Vec<PublicResultBoardRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT r.id, r.athlete_id, a.full_name, r.competition_id, c.title, \
             r.snatch, r.clean_and_jerk, r.total, r.date, r.squat_kg, r.bench_kg, r.deadlift_kg \
             FROM results r \
             INNER JOIN athletes a ON a.id = r.athlete_id \
             LEFT JOIN competitions c ON c.id = r.competition_id \
             WHERE r.status = 'Approved' \
             ORDER BY r.date DESC, r.total DESC",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let competition_title = sql_row::opt_string(&row, 4).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        out.push(PublicResultBoardRow {
            id: sql_row::required_string(&row, 0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_id: sql_row::required_string(&row, 1).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_name: sql_row::required_string(&row, 2).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            competition_id: sql_row::opt_string(&row, 3).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            competition_title,
            snatch: sql_row::required_f64(&row, 5).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            clean_and_jerk: sql_row::required_f64(&row, 6).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            total: sql_row::required_f64(&row, 7).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            date: sql_row::required_string(&row, 8).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            squat_kg: sql_row::opt_f64(&row, 9).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            bench_kg: sql_row::opt_f64(&row, 10).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            deadlift_kg: sql_row::opt_f64(&row, 11).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        });
    }

    Ok(Json(out))
}

/// Publiczna tablica klasycznego dwuboju (bez wpisów stricte siłowych).
pub async fn list_public_olympic_board(
    State(state): State<AppState>,
) -> Result<Json<Vec<PublicResultBoardRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT r.id, r.athlete_id, a.full_name, r.competition_id, c.title, \
             r.snatch, r.clean_and_jerk, r.total, r.date, r.squat_kg, r.bench_kg, r.deadlift_kg \
             FROM results r \
             INNER JOIN athletes a ON a.id = r.athlete_id \
             LEFT JOIN competitions c ON c.id = r.competition_id \
             WHERE r.status = 'Approved' AND r.total >= 20 \
             ORDER BY r.date DESC, r.total DESC",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        out.push(PublicResultBoardRow {
            id: sql_row::required_string(&row, 0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_id: sql_row::required_string(&row, 1).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_name: sql_row::required_string(&row, 2).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            competition_id: sql_row::opt_string(&row, 3).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            competition_title: sql_row::opt_string(&row, 4).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            snatch: sql_row::required_f64(&row, 5).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            clean_and_jerk: sql_row::required_f64(&row, 6).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            total: sql_row::required_f64(&row, 7).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?,
            date: sql_row::required_string(&row, 8).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            squat_kg: sql_row::opt_f64(&row, 9).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            bench_kg: sql_row::opt_f64(&row, 10).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            deadlift_kg: sql_row::opt_f64(&row, 11).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        });
    }
    Ok(Json(out))
}

pub async fn list_approved_results(
    State(state): State<AppState>,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg FROM results WHERE status = 'Approved'", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

pub async fn list_pending_results(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg FROM results WHERE status = 'Pending'", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

/// Publiczne karty / ranking / wykresy — wyłącznie zatwierdzone zgłoszenia.
pub async fn list_athlete_results(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT r.id, r.athlete_id, r.snatch, r.clean_and_jerk, r.total, r.status, r.date, r.squat_kg, r.bench_kg, r.deadlift_kg \
             FROM results r \
             INNER JOIN athletes a ON a.id = r.athlete_id AND (a.is_active IS NULL OR a.is_active = 1) \
             WHERE r.athlete_id = ?1 AND r.status = 'Approved' ORDER BY r.date ASC",
            [athlete_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

/// Zawodnik (własny profil) lub kadra — wszystkie zgłoszenia (Pending + Approved).
pub async fn list_athlete_result_submissions(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    claims: Claims,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    ensure_can_view_athlete_submissions(&state, &claims, &athlete_id).await?;

    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg FROM results \
             WHERE athlete_id = ?1 ORDER BY date DESC, id DESC",
            [athlete_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let r = competition_result_from_row(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        results.push(r);
    }

    Ok(Json(results))
}

pub async fn create_result(
    State(state): State<AppState>,
    claims: Claims, // Must be authenticated
    Json(payload): Json<CreateResultRequest>,
) -> Result<Json<CompetitionResult>, ApiError> {
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
        let mut rows = state.db.query("SELECT id FROM athletes WHERE user_id = ?1", [claims.sub])
            .await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
            let athlete_id = sql_row::required_string(&row, 0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            if athlete_id != payload.athlete_id {
                return Err(api_error(StatusCode::FORBIDDEN, "Athletes can only submit their own results"));
            }
        } else {
            return Err(api_error(StatusCode::NOT_FOUND, "Athlete profile not found"));
        }
    }

    let id = Uuid::new_v4().to_string();
    let squat_kg = payload.squat_kg.filter(|x| *x > 0.0);
    let bench_kg = payload.bench_kg.filter(|x| *x > 0.0);
    let deadlift_kg = payload.deadlift_kg.filter(|x| *x > 0.0);

    state.db.execute(
        "INSERT INTO results (id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        (
            id.clone(),
            payload.athlete_id.clone(),
            payload.snatch,
            payload.clean_and_jerk,
            payload.total,
            status.to_string(),
            payload.date.clone(),
            squat_kg,
            bench_kg,
            deadlift_kg,
        ),
    ).await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    sync_athlete_bests_from_approved(&state, &payload.athlete_id).await?;

    if status == ResultStatus::Pending {
        let name = crate::notifications::athlete_full_name(state.db.as_ref(), &payload.athlete_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "Zawodnik".to_string());
        crate::notifications::notify_result_pending(
            &state,
            &payload.athlete_id,
            &name,
            payload.total,
            &payload.date,
        );
    }

    Ok(Json(CompetitionResult {
        id,
        athlete_id: payload.athlete_id,
        snatch: payload.snatch,
        clean_and_jerk: payload.clean_and_jerk,
        total: payload.total,
        status,
        date: payload.date,
        squat_kg,
        bench_kg,
        deadlift_kg,
    }))
}

pub async fn approve_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
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
    let athlete_id: String = row.get(0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let total: f64 = row.get(1).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let date: String = row.get(2).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let n = state
        .db
        .execute("UPDATE results SET status = 'Approved' WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Result not found"));
    }
    sync_athlete_bests_from_approved(&state, &athlete_id).await?;
    crate::notifications::notify_result_approved(&state, &athlete_id, total, &date);
    Ok(StatusCode::OK)
}

pub async fn reject_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
) -> Result<StatusCode, ApiError> {
    let n = state
        .db
        .execute("UPDATE results SET status = 'Rejected' WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if n == 0 {
        return Err(api_error(StatusCode::NOT_FOUND, "Result not found"));
    }
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
}

pub async fn list_all_results_staff(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<CompetitionResult>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg FROM results ORDER BY date DESC",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
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
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "At least one field is required",
        ));
    }

    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg FROM results WHERE id = ?1",
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
        cr.snatch = v;
    }
    if let Some(v) = payload.clean_and_jerk {
        cr.clean_and_jerk = v;
    }
    if let Some(v) = payload.total {
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

    state
        .db
        .execute(
            "UPDATE results SET snatch = ?1, clean_and_jerk = ?2, total = ?3, status = ?4, date = ?5, squat_kg = ?6, bench_kg = ?7, deadlift_kg = ?8 WHERE id = ?9",
            (
                cr.snatch,
                cr.clean_and_jerk,
                cr.total,
                cr.status.to_string(),
                cr.date.clone(),
                cr.squat_kg,
                cr.bench_kg,
                cr.deadlift_kg,
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
