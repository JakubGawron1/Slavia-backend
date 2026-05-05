use axum::{extract::State, http::StatusCode, Json};

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::Claims;
use crate::models::ExerciseBoardRow;
use crate::state::AppState;

pub async fn list_exercises_board(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Vec<ExerciseBoardRow>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT
                a.id,
                a.full_name,
                (
                  SELECT r.squat_kg
                  FROM results r
                  WHERE r.athlete_id = a.id
                    AND r.status = 'Approved'
                    AND r.squat_kg IS NOT NULL
                  ORDER BY r.date DESC, r.id DESC
                  LIMIT 1
                ) AS squat_kg,
                (
                  SELECT r.bench_kg
                  FROM results r
                  WHERE r.athlete_id = a.id
                    AND r.status = 'Approved'
                    AND r.bench_kg IS NOT NULL
                  ORDER BY r.date DESC, r.id DESC
                  LIMIT 1
                ) AS bench_kg,
                (
                  SELECT r.deadlift_kg
                  FROM results r
                  WHERE r.athlete_id = a.id
                    AND r.status = 'Approved'
                    AND r.deadlift_kg IS NOT NULL
                  ORDER BY r.date DESC, r.id DESC
                  LIMIT 1
                ) AS deadlift_kg,
                (
                  SELECT COUNT(1)
                  FROM results rp
                  WHERE rp.athlete_id = a.id
                    AND rp.status = 'Pending'
                    AND (rp.squat_kg IS NOT NULL OR rp.bench_kg IS NOT NULL OR rp.deadlift_kg IS NOT NULL)
                ) AS pending_strength_count,
                (
                  SELECT COUNT(1)
                  FROM results ra
                  WHERE ra.athlete_id = a.id
                    AND ra.status = 'Approved'
                    AND (ra.squat_kg IS NOT NULL OR ra.bench_kg IS NOT NULL OR ra.deadlift_kg IS NOT NULL)
                ) AS approved_strength_count,
                (
                  SELECT COUNT(1)
                  FROM training_log_entries t
                  WHERE t.athlete_id = a.id
                ) AS training_log_count,
                (
                  SELECT MAX(ra.date)
                  FROM results ra
                  WHERE ra.athlete_id = a.id
                    AND ra.status = 'Approved'
                    AND (ra.squat_kg IS NOT NULL OR ra.bench_kg IS NOT NULL OR ra.deadlift_kg IS NOT NULL)
                ) AS last_approved_date
             FROM athletes a
             WHERE a.is_active = 1
             ORDER BY a.full_name ASC",
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
        let squat_kg: Option<f64> = row.get(2).ok();
        let bench_kg: Option<f64> = row.get(3).ok();
        let deadlift_kg: Option<f64> = row.get(4).ok();
        let pending_count: i64 = row.get(5).unwrap_or(0_i64);
        let approved_count: i64 = row.get(6).unwrap_or(0_i64);
        let training_log_count: i64 = row.get(7).unwrap_or(0_i64);
        let last_date: Option<String> = row.get(8).ok();

        out.push(ExerciseBoardRow {
            athlete_id: row
                .get::<String>(0)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            athlete_name: row
                .get::<String>(1)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            squat_kg,
            bench_kg,
            deadlift_kg,
            source_trainer_direct: approved_count > 0,
            source_athlete_pending_count: pending_count,
            source_approved_results_count: approved_count,
            source_training_log_count: training_log_count,
            source_last_approved_date: last_date,
        });
    }

    Ok(Json(out))
}
