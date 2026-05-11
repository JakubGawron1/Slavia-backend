use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::{Claims, claims_has_staff_access};
use crate::models::{ResultStatus, Role};
use crate::sql_row;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ExerciseSubmissionDto {
    pub id: String,
    pub athlete_id: String,
    pub athlete_name: Option<String>,
    pub exercise_id: String,
    pub exercise_name: String,
    pub value: f64,
    pub unit: String,
    pub performed_at: String,
    pub notes: Option<String>,
    pub status: ResultStatus,
    pub reviewed_at: Option<String>,
    pub review_note: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateExerciseSubmissionRequest {
    pub exercise_id: String,
    pub value: f64,
    pub performed_at: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewExerciseSubmissionRequest {
    #[serde(default)]
    pub review_note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExerciseBoardQuery {
    pub exercise_id: String,
}

#[derive(Debug, Serialize)]
pub struct ExerciseBoardRowDto {
    pub athlete_id: String,
    pub athlete_name: String,
    pub best_value: f64,
    pub unit: String,
    pub entries: i64,
    pub last_performed_at: Option<String>,
}

async fn my_athlete_id_for_claims(state: &AppState, claims: &Claims) -> Result<String, ApiError> {
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
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Brak profilu zawodnika"))?;
    sql_row::required_string(&row, 0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))
}

pub async fn create_exercise_submission(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<CreateExerciseSubmissionRequest>,
) -> Result<Json<ExerciseSubmissionDto>, ApiError> {
    if !claims.roles.contains(&Role::Athlete) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Tylko zawodnik może zgłaszać wyniki",
        ));
    }
    if payload.exercise_id.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "exercise_id is required",
        ));
    }
    if !payload.value.is_finite() || payload.value <= 0.0 {
        return Err(api_error(StatusCode::BAD_REQUEST, "value must be > 0"));
    }
    if payload.performed_at.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "performed_at is required",
        ));
    }

    // Ensure athlete exists and is linked to this user.
    let athlete_id = my_athlete_id_for_claims(&state, &claims).await?;

    // Ensure exercise exists.
    let mut ex_rows = state
        .db
        .query(
            "SELECT name FROM exercises WHERE id = ?1",
            [payload.exercise_id.trim().to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let ex_row = ex_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "Ćwiczenie nie istnieje"))?;
    let exercise_name = sql_row::required_string(&ex_row, 0)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let notes = payload.notes.and_then(|s| {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    });

    state
        .db
        .execute(
            "INSERT INTO exercise_submissions (
                id, athlete_id, exercise_id, value, unit, performed_at,
                notes, status, reviewed_by_user_id, reviewed_at, review_note, created_at
             ) VALUES (?1, ?2, ?3, ?4, 'kg', ?5, ?6, 'Pending', NULL, NULL, NULL, ?7)",
            (
                id.clone(),
                athlete_id.clone(),
                payload.exercise_id.trim().to_string(),
                payload.value,
                payload.performed_at.trim().to_string(),
                notes.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ExerciseSubmissionDto {
        id,
        athlete_id,
        athlete_name: None,
        exercise_id: payload.exercise_id,
        exercise_name,
        value: payload.value,
        unit: "kg".to_string(),
        performed_at: payload.performed_at,
        notes,
        status: ResultStatus::Pending,
        reviewed_at: None,
        review_note: None,
        created_at: now,
    }))
}

pub async fn list_my_exercise_submissions(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Vec<ExerciseSubmissionDto>>, ApiError> {
    if !claims.roles.contains(&Role::Athlete) {
        return Err(api_error(StatusCode::FORBIDDEN, "Tylko zawodnik"));
    }
    let athlete_id = my_athlete_id_for_claims(&state, &claims).await?;

    let mut rows = state
        .db
        .query(
            "SELECT s.id, s.athlete_id, e.id, e.name, s.value, s.unit, s.performed_at, s.notes,
                    s.status, s.reviewed_at, s.review_note, s.created_at
             FROM exercise_submissions s
             JOIN exercises e ON e.id = s.exercise_id
             WHERE s.athlete_id = ?1
             ORDER BY s.created_at DESC",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let status_str = sql_row::required_string(&row, 8)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        let status = status_str
            .parse::<ResultStatus>()
            .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Invalid status"))?;
        out.push(ExerciseSubmissionDto {
            id: sql_row::required_string(&row, 0)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_id: sql_row::required_string(&row, 1)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_name: None,
            exercise_id: sql_row::required_string(&row, 2)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            exercise_name: sql_row::required_string(&row, 3)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            value: sql_row::required_f64(&row, 4)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            unit: sql_row::required_string(&row, 5)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            performed_at: sql_row::required_string(&row, 6)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            notes: sql_row::opt_string(&row, 7)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            status,
            reviewed_at: sql_row::opt_string(&row, 9)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            review_note: sql_row::opt_string(&row, 10)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            created_at: sql_row::required_string(&row, 11)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        });
    }
    Ok(Json(out))
}

pub async fn list_pending_exercise_submissions(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Vec<ExerciseSubmissionDto>>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień"));
    }
    let mut rows = state
        .db
        .query(
            "SELECT s.id, s.athlete_id, a.full_name, e.id, e.name, s.value, s.unit, s.performed_at, s.notes,
                    s.status, s.reviewed_at, s.review_note, s.created_at
             FROM exercise_submissions s
             JOIN athletes a ON a.id = s.athlete_id
             JOIN exercises e ON e.id = s.exercise_id
             WHERE s.status = 'Pending'
             ORDER BY s.created_at DESC",
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
        out.push(ExerciseSubmissionDto {
            id: sql_row::required_string(&row, 0)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_id: sql_row::required_string(&row, 1)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_name: Some(
                sql_row::required_string(&row, 2)
                    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            ),
            exercise_id: sql_row::required_string(&row, 3)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            exercise_name: sql_row::required_string(&row, 4)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            value: sql_row::required_f64(&row, 5)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            unit: sql_row::required_string(&row, 6)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            performed_at: sql_row::required_string(&row, 7)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            notes: sql_row::opt_string(&row, 8)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            status: ResultStatus::Pending,
            reviewed_at: sql_row::opt_string(&row, 10)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            review_note: sql_row::opt_string(&row, 11)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            created_at: sql_row::required_string(&row, 12)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        });
    }
    Ok(Json(out))
}

async fn load_submission_for_review(
    state: &AppState,
    id: &str,
) -> Result<
    (
        String,
        String,
        f64,
        String,
        String,
        Option<String>,
        ResultStatus,
    ),
    ApiError,
> {
    let mut rows = state
        .db
        .query(
            "SELECT athlete_id, exercise_id, value, unit, performed_at, notes, status
             FROM exercise_submissions WHERE id = ?1",
            [id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Zgłoszenie nie istnieje"))?;

    let status_str = sql_row::required_string(&row, 6)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let status = status_str
        .parse::<ResultStatus>()
        .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Invalid status"))?;

    Ok((
        sql_row::required_string(&row, 0)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        sql_row::required_string(&row, 1)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        sql_row::required_f64(&row, 2)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        sql_row::required_string(&row, 3)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        sql_row::required_string(&row, 4)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        sql_row::opt_string(&row, 5)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        status,
    ))
}

pub async fn approve_exercise_submission(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
    Json(payload): Json<ReviewExerciseSubmissionRequest>,
) -> Result<StatusCode, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień"));
    }
    let (athlete_id, exercise_id, value, unit, performed_at, notes, status) =
        load_submission_for_review(&state, &id).await?;
    if status == ResultStatus::Approved {
        return Ok(StatusCode::NO_CONTENT);
    }
    if status == ResultStatus::Rejected {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Nie można zatwierdzić odrzuconego zgłoszenia",
        ));
    }

    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let review_note = payload.review_note.and_then(|s| {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    });

    // 1) Update submission
    state
        .db
        .execute(
            "UPDATE exercise_submissions
             SET status = 'Approved', reviewed_by_user_id = ?1, reviewed_at = ?2, review_note = ?3
             WHERE id = ?4",
            (
                claims.sub.clone(),
                now.clone(),
                review_note.clone(),
                id.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 2) Insert history (source of ranking)
    let hid = Uuid::new_v4().to_string();
    state
        .db
        .execute(
            "INSERT INTO exercise_results_history (
                id, athlete_id, exercise_id, value, unit, performed_at,
                submission_id, approved_by_user_id, approved_at, notes, review_note, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            (
                hid,
                athlete_id,
                exercise_id,
                value,
                unit,
                performed_at,
                Some(id),
                Some(claims.sub),
                now.clone(),
                notes,
                review_note,
                now,
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn reject_exercise_submission(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
    Json(payload): Json<ReviewExerciseSubmissionRequest>,
) -> Result<StatusCode, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(StatusCode::FORBIDDEN, "Brak uprawnień"));
    }
    let (_, _, _, _, _, _, status) = load_submission_for_review(&state, &id).await?;
    if status == ResultStatus::Rejected {
        return Ok(StatusCode::NO_CONTENT);
    }
    if status == ResultStatus::Approved {
        return Err(api_error(
            StatusCode::CONFLICT,
            "Nie można odrzucić zatwierdzonego zgłoszenia",
        ));
    }

    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let review_note = payload.review_note.and_then(|s| {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    });

    state
        .db
        .execute(
            "UPDATE exercise_submissions
             SET status = 'Rejected', reviewed_by_user_id = ?1, reviewed_at = ?2, review_note = ?3
             WHERE id = ?4",
            (claims.sub, now, review_note, id),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn exercise_board_for_exercise(
    State(state): State<AppState>,
    _claims: Claims,
    Query(q): Query<ExerciseBoardQuery>,
) -> Result<Json<Vec<ExerciseBoardRowDto>>, ApiError> {
    if q.exercise_id.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "exercise_id is required",
        ));
    }
    let ex_id = q.exercise_id.trim().to_string();

    let mut rows = state
        .db
        .query(
            "SELECT h.athlete_id, a.full_name,
                    MAX(h.value) AS best_value,
                    MIN(h.unit) AS unit,
                    COUNT(*) AS entries,
                    MAX(h.performed_at) AS last_performed_at
             FROM exercise_results_history h
             JOIN athletes a ON a.id = h.athlete_id
             WHERE h.exercise_id = ?1
             GROUP BY h.athlete_id, a.full_name
             HAVING best_value > 0
             ORDER BY best_value DESC, last_performed_at DESC",
            [ex_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(ExerciseBoardRowDto {
            athlete_id: sql_row::required_string(&row, 0)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            athlete_name: sql_row::required_string(&row, 1)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            best_value: sql_row::required_f64(&row, 2)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            unit: sql_row::required_string(&row, 3)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?,
            entries: row.get::<i64>(4).unwrap_or(0),
            last_performed_at: sql_row::opt_string(&row, 5)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        });
    }
    Ok(Json(out))
}
