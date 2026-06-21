use axum::{
    Json,
    extract::State,
    http::StatusCode,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::Claims;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct VoteRequest {
    pub athlete_id: String,
}

#[derive(Serialize)]
pub struct MyVoteResponse {
    pub athlete_id: Option<String>,
    pub athlete_name: Option<String>,
}

#[derive(Serialize)]
pub struct VoteSummaryRow {
    pub athlete_id: String,
    pub athlete_name: String,
    pub votes_count: i64,
}

fn row_api_err(e: libsql::Error) -> ApiError {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn vote_summary_from_row(row: &libsql::Row) -> Result<VoteSummaryRow, ApiError> {
    Ok(VoteSummaryRow {
        athlete_id: row.get(0).map_err(row_api_err)?,
        athlete_name: row.get(1).map_err(row_api_err)?,
        votes_count: row.get(2).map_err(row_api_err)?,
    })
}

pub async fn submit_vote(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<VoteRequest>,
) -> Result<StatusCode, ApiError> {
    let month = Utc::now().format("%Y-%m").to_string();
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    let n = state
        .db
        .execute(
            "INSERT INTO club_votes (id, voter_user_id, athlete_id, month, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            (id, claims.sub, payload.athlete_id, month, now),
        )
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint failed") {
                api_error(StatusCode::BAD_REQUEST, "Już oddałeś głos w tym miesiącu")
            } else {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    if n == 0 {
        return Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to vote"));
    }

    Ok(StatusCode::CREATED)
}

pub async fn get_my_vote(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<MyVoteResponse>, ApiError> {
    let month = Utc::now().format("%Y-%m").to_string();
    let mut rows = state
        .db
        .query(
            "SELECT v.athlete_id, a.full_name FROM club_votes v \
             JOIN athletes a ON v.athlete_id = a.id \
             WHERE v.voter_user_id = ?1 AND v.month = ?2 LIMIT 1",
            (claims.sub, month),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        Ok(Json(MyVoteResponse {
            athlete_id: Some(row.get(0).map_err(row_api_err)?),
            athlete_name: Some(row.get(1).map_err(row_api_err)?),
        }))
    } else {
        Ok(Json(MyVoteResponse {
            athlete_id: None,
            athlete_name: None,
        }))
    }
}

pub async fn get_vote_summary(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Vec<VoteSummaryRow>>, ApiError> {
    let month = Utc::now().format("%Y-%m").to_string();
    let mut rows = state
        .db
        .query(
            "SELECT a.id, a.full_name, COUNT(v.id) as cnt FROM athletes a \
             LEFT JOIN club_votes v ON a.id = v.athlete_id AND v.month = ?1 \
             GROUP BY a.id HAVING cnt > 0 \
             ORDER BY cnt DESC",
            [month],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(vote_summary_from_row(&row)?);
    }

    Ok(Json(out))
}
