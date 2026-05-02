use std::collections::HashSet;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use chrono::{SecondsFormat, Utc};

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::{Claims, RequireTrainerOrHigher};
use crate::models::{Competition, Role};
use crate::state::AppState;

#[derive(Serialize)]
pub struct ParticipantBrief {
    pub athlete_id: String,
    pub full_name: String,
}

#[derive(Deserialize)]
pub struct SetParticipantsRequest {
    pub athlete_ids: Vec<String>,
}

#[derive(Deserialize)]
pub struct SyncAthleteCompetitionAssignmentsRequest {
    pub competition_ids: Vec<String>,
}

pub async fn list_competitions_for_athlete(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<Vec<Competition>>, ApiError> {
    let mut chk = state
        .db
        .query("SELECT id FROM athletes WHERE id = ?1", [athlete_id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if chk
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"));
    }

    let mut rows = state
        .db
        .query(
            "SELECT c.id, c.title, c.date, c.location, c.description, c.category, c.status, \
                    c.external_source, c.external_ref, c.external_url \
             FROM competitions c \
             INNER JOIN competition_participants p ON p.competition_id = c.id AND p.athlete_id = ?1 \
             ORDER BY c.date ASC",
            [athlete_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut list = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        list.push(Competition {
            id: row.get(0).unwrap(),
            title: row.get(1).unwrap(),
            date: row.get(2).unwrap(),
            location: row.get(3).unwrap(),
            description: row.get(4).ok(),
            category: row.get(5).ok(),
            status: row.get(6).ok(),
            external_source: row.get(7).ok(),
            external_ref: row.get(8).ok(),
            external_url: row.get(9).ok(),
        });
    }

    Ok(Json(list))
}

pub async fn sync_competitions_for_athlete(
    State(state): State<AppState>,
    Path(athlete_id): Path<String>,
    auth: RequireTrainerOrHigher,
    Json(payload): Json<SyncAthleteCompetitionAssignmentsRequest>,
) -> Result<StatusCode, ApiError> {
    let claims = &auth.0;

    let mut chk = state
        .db
        .query("SELECT id FROM athletes WHERE id = ?1", [athlete_id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if chk
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(StatusCode::NOT_FOUND, "Athlete not found"));
    }

    let mut prev_rows = state
        .db
        .query(
            "SELECT competition_id FROM competition_participants WHERE athlete_id = ?1",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut old_ids = HashSet::<String>::new();
    while let Some(row) = prev_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let cid: String = row.get(0).unwrap();
        old_ids.insert(cid);
    }

    state
        .db
        .execute(
            "DELETE FROM competition_participants WHERE athlete_id = ?1",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let assigned_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    let mut seen = std::collections::HashSet::new();
    let mut new_ids = HashSet::<String>::new();
    for cid in payload.competition_ids {
        if !seen.insert(cid.clone()) {
            continue;
        }

        let mut ok = state
            .db
            .query("SELECT id FROM competitions WHERE id = ?1", [cid.clone()])
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if ok
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_none()
        {
            continue;
        }

        new_ids.insert(cid.clone());

        state
            .db
            .execute(
                "INSERT INTO competition_participants (competition_id, athlete_id, assigned_at, assigned_by) VALUES (?1, ?2, ?3, ?4)",
                (
                    cid,
                    athlete_id.clone(),
                    assigned_at.clone(),
                    claims.sub.clone(),
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    crate::notifications::diff_notify_athlete_competition_assignments(&state, &athlete_id, old_ids, new_ids).await;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_participants(
    State(state): State<AppState>,
    Path(competition_id): Path<String>,
    _claims: Claims,
) -> Result<Json<Vec<ParticipantBrief>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT a.id, a.full_name FROM competition_participants cp \
             JOIN athletes a ON a.id = cp.athlete_id \
             WHERE cp.competition_id = ?1 \
             ORDER BY a.full_name ASC",
            [competition_id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        out.push(ParticipantBrief {
            athlete_id: row.get(0).unwrap(),
            full_name: row.get(1).unwrap(),
        });
    }

    Ok(Json(out))
}

pub async fn set_participants(
    State(state): State<AppState>,
    Path(competition_id): Path<String>,
    auth: RequireTrainerOrHigher,
    Json(payload): Json<SetParticipantsRequest>,
) -> Result<StatusCode, ApiError> {
    let claims = &auth.0;
    let mut check = state
        .db
        .query(
            "SELECT id FROM competitions WHERE id = ?1",
            [competition_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if check
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(StatusCode::NOT_FOUND, "Competition not found"));
    }

    let comp_title = crate::notifications::competition_title(state.db.as_ref(), &competition_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "Zawody".to_string());

    let mut old_pa_rows = state
        .db
        .query(
            "SELECT athlete_id FROM competition_participants WHERE competition_id = ?1",
            [competition_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut old_athlete_ids = HashSet::<String>::new();
    while let Some(row) = old_pa_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        old_athlete_ids.insert(row.get(0).unwrap());
    }

    state
        .db
        .execute(
            "DELETE FROM competition_participants WHERE competition_id = ?1",
            [competition_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let assigned_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    let mut new_athlete_ids = HashSet::<String>::new();
    for aid in payload.athlete_ids {
        let mut ok = state
            .db
            .query(
                "SELECT id FROM athletes WHERE id = ?1 AND (is_active IS NULL OR is_active = 1)",
                [aid.clone()],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if ok
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_none()
        {
            continue;
        }
        new_athlete_ids.insert(aid.clone());
        state
            .db
            .execute(
                "INSERT INTO competition_participants (competition_id, athlete_id, assigned_at, assigned_by) VALUES (?1, ?2, ?3, ?4)",
                (
                    competition_id.clone(),
                    aid,
                    assigned_at.clone(),
                    claims.sub.clone(),
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    for added in new_athlete_ids.difference(&old_athlete_ids) {
        crate::notifications::notify_competition_assigned_to_athlete(&state, added, &competition_id, &comp_title);
    }
    for removed in old_athlete_ids.difference(&new_athlete_ids) {
        crate::notifications::notify_competition_unassigned_from_athlete(&state, removed, &competition_id, &comp_title);
    }
    crate::notifications::notify_competition_roster_updated(&state, &competition_id, &comp_title);

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct MyCalendarEntry {
    pub competition: Competition,
    pub participants: Vec<ParticipantBrief>,
}

#[derive(Serialize)]
pub struct MyCalendarResponse {
    pub entries: Vec<MyCalendarEntry>,
}

pub async fn my_calendar_for_athlete(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<MyCalendarResponse>, ApiError> {
    if claims.role != Role::Athlete {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Only athletes can use this endpoint",
        ));
    }

    let mut rows = state
        .db
        .query(
            "SELECT id FROM athletes WHERE user_id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let athlete_row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let athlete_id: String = match athlete_row {
        Some(r) => r.get(0).unwrap(),
        None => {
            return Ok(Json(MyCalendarResponse {
                entries: vec![],
            }))
        }
    };

    let mut rows = state
        .db
        .query(
            "SELECT c.id, c.title, c.date, c.location, c.description, c.category, c.status, \
                    c.external_source, c.external_ref, c.external_url \
             FROM competitions c \
             INNER JOIN competition_participants p ON p.competition_id = c.id \
             WHERE p.athlete_id = ?1 \
             ORDER BY c.date ASC",
            [athlete_id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut entries = Vec::new();

    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let cid: String = row.get(0).unwrap();
        let competition = Competition {
            id: cid.clone(),
            title: row.get(1).unwrap(),
            date: row.get(2).unwrap(),
            location: row.get(3).unwrap(),
            description: row.get(4).ok(),
            category: row.get(5).ok(),
            status: row.get(6).ok(),
            external_source: row.get(7).ok(),
            external_ref: row.get(8).ok(),
            external_url: row.get(9).ok(),
        };

        let mut pr = state
            .db
            .query(
                "SELECT a.id, a.full_name FROM competition_participants cp \
                 JOIN athletes a ON a.id = cp.athlete_id \
                 WHERE cp.competition_id = ?1 \
                 ORDER BY a.full_name ASC",
                [cid.clone()],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut participants = Vec::new();
        while let Some(r) = pr
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            participants.push(ParticipantBrief {
                athlete_id: r.get(0).unwrap(),
                full_name: r.get(1).unwrap(),
            });
        }

        entries.push(MyCalendarEntry {
            competition,
            participants,
        });
    }

    Ok(Json(MyCalendarResponse { entries }))
}
