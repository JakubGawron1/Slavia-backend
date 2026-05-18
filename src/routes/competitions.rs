use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use libsql::Row;
use serde::Deserialize;
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::external_calendar_sync;
use crate::middleware::auth::{RequireAdminOrSuperAdmin, RequireTrainerOrHigher};
use crate::models::Competition;
use crate::notifications;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateCompetitionRequest {
    pub title: String,
    pub date: String,
    pub location: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub status: Option<String>,
}

fn competition_from_row(row: &Row) -> Result<Competition, libsql::Error> {
    Ok(Competition {
        id: row.get(0)?,
        title: row.get(1)?,
        date: row.get(2)?,
        location: row.get(3)?,
        description: row.get(4).ok(),
        category: row.get(5).ok(),
        status: row.get(6).ok(),
        external_source: row.get(7).ok(),
        external_ref: row.get(8).ok(),
        external_url: row.get(9).ok(),
    })
}

pub async fn list_competitions(
    State(state): State<AppState>,
) -> Result<Json<Vec<Competition>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, title, date, location, description, COALESCE(category_override, category) AS category, status, external_source, external_ref, external_url \
             FROM competitions ORDER BY date ASC",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut competitions = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        competitions.push(
            competition_from_row(&row)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
    }

    Ok(Json(competitions))
}

pub async fn create_competition(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
    Json(payload): Json<CreateCompetitionRequest>,
) -> Result<Json<Competition>, ApiError> {
    let id = Uuid::new_v4().to_string();
    let category = payload
        .category
        .clone()
        .unwrap_or_else(|| "club_event".to_string());
    let status = payload
        .status
        .clone()
        .unwrap_or_else(|| "scheduled".to_string());

    state
        .db
        .execute(
            "INSERT INTO competitions (id, title, date, location, description, category, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                id.clone(),
                payload.title.clone(),
                payload.date.clone(),
                payload.location.clone(),
                payload.description.clone(),
                category.clone(),
                status.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_competition_created(
        &state,
        &payload.title,
        &payload.date,
        &payload.location,
        &id,
    );

    Ok(Json(Competition {
        id,
        title: payload.title,
        date: payload.date,
        location: payload.location,
        description: payload.description,
        category: Some(category),
        status: Some(status),
        external_source: None,
        external_ref: None,
        external_url: None,
    }))
}

async fn competition_external_source(
    state: &AppState,
    id: &str,
) -> Result<Option<String>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT external_source FROM competitions WHERE id = ?1",
            [id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match row {
        Some(r) => Ok(r.get(0).ok()),
        None => Err(api_error(StatusCode::NOT_FOUND, "Competition not found")),
    }
}

pub async fn delete_competition(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
) -> Result<StatusCode, ApiError> {
    if competition_external_source(&state, &id).await?.is_some() {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Zawody zsynchronizowane z PZPC lub PodnoszenieCiezarow.pl można tylko aktualizować przyciskiem synchronizacji — nie usuwaj ich ręcznie.",
        ));
    }

    let conn_arc = state.db.raw().await;
    let title_for_notify = notifications::competition_title(conn_arc.as_ref(), &id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| id.clone());

    state
        .db
        .execute("DELETE FROM competitions WHERE id = ?1", [id.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_competition_deleted(&state, &title_for_notify, &id);

    Ok(StatusCode::NO_CONTENT)
}

pub async fn update_competition(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _auth: RequireTrainerOrHigher,
    Json(payload): Json<CreateCompetitionRequest>,
) -> Result<Json<Competition>, ApiError> {
    let ext = competition_external_source(&state, &id).await?;

    // [2027] Read old values to detect schedule changes
    let old_vals: Option<(String, String)> = {
        let mut rows = state
            .db
            .query("SELECT date, location FROM competitions WHERE id = ?1", [id.clone()])
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            let d: String = row.get(0).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let l: String = row.get(1).map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Some((d, l))
        } else {
            None
        }
    };

    if ext.is_some() {
        let status = payload.status.unwrap_or_else(|| "scheduled".to_string());
        let category_opt = payload.category.map(|s| s.trim().to_string());
        let category_override: Option<String> = category_opt
            .as_deref()
            .map(|s| s.trim())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        state
            .db
            .execute(
                "UPDATE competitions SET status = ?1, category_override = ?2 WHERE id = ?3",
                (status.clone(), category_override, id.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else {
        let category = payload.category.unwrap_or_else(|| "club_event".to_string());
        let status = payload.status.unwrap_or_else(|| "scheduled".to_string());
        state
            .db
            .execute(
                "UPDATE competitions SET title = ?1, date = ?2, location = ?3, description = ?4, category = ?5, status = ?6 WHERE id = ?7",
                (
                    payload.title.clone(),
                    payload.date.clone(),
                    payload.location.clone(),
                    payload.description.clone(),
                    category,
                    status.clone(),
                    id.clone(),
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let mut rows = state
        .db
        .query(
            "SELECT id, title, date, location, description, COALESCE(category_override, category) AS category, status, external_source, external_ref, external_url FROM competitions WHERE id = ?1",
            [id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Competition not found"))?;

    let updated = competition_from_row(&row)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    notifications::notify_competition_updated(&state, &updated.title, &updated.id);

    // [2027] Notify assigned athletes if date or location changed
    if ext.is_none() {
        if let Some((old_date, old_location)) = old_vals {
            let date_changed = old_date != payload.date;
            let location_changed = old_location != payload.location;
            if date_changed || location_changed {
                notifications::notify_competition_schedule_changed(
                    &state,
                    &updated.id,
                    &updated.title,
                    &updated.date,
                    &updated.location,
                );
            }
        }
    }

    Ok(Json(updated))
}

pub async fn sync_external_competitions(
    State(state): State<AppState>,
    _auth: RequireAdminOrSuperAdmin,
) -> Result<Json<external_calendar_sync::SyncExternalResponse>, ApiError> {
    let conn_arc = state.db.raw().await;
    let r = external_calendar_sync::run_sync(conn_arc.as_ref()).await?;
    notifications::notify_competitions_synced(
        &state,
        r.pzpc_imported,
        r.pc_imported,
        r.upserts,
        r.stale_removed,
        r.stale_import_removed,
        r.stale_manual_removed,
    );
    Ok(Json(r))
}
