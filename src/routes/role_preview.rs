//! SuperAdmin: podgląd panelu jako inny użytkownik (read-only, audytowany).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::dto::notifications::NotificationDto;
use crate::middleware::auth::RequireSuperAdmin;
use crate::models::{Athlete, Role};
use crate::pagination::ListPaginationQuery;
use crate::repos;
use crate::routes::admins::user_roles_by_id;
use crate::routes::athletes::fetch_athlete_by_user_id;
use crate::routes::chat::{ChatMessageDto, ChatThreadDto, messages_for_user, threads_for_user};
use crate::routes::competition_participants::{
    MyCalendarResponse, calendar_entries_for_athlete_id,
};
use crate::routes::payments::{MonthQuery, PaymentStatusResponse, payment_status_for_athlete_id};
use crate::routes::results::competition_result_from_row;
use crate::state::AppState;

#[derive(Serialize)]
pub struct RolePreviewContextDto {
    pub user_id: String,
    pub username: String,
    pub roles: Vec<String>,
    pub preview_roles: Vec<String>,
    pub athlete_id: Option<String>,
    pub athlete_name: Option<String>,
}

#[derive(Deserialize)]
pub struct RolePreviewSessionRequest {
    pub action: String,
    pub target_user_id: String,
    pub preview_role: String,
}

#[derive(Serialize)]
pub struct RolePreviewAthleteBundleDto {
    pub athlete: Option<Athlete>,
    pub calendar_entries: Vec<crate::routes::competition_participants::MyCalendarEntry>,
    pub results: Vec<crate::models::CompetitionResult>,
}

fn preview_roles_for_user(roles: &[Role]) -> Vec<String> {
    let mut out = Vec::new();
    if roles.iter().any(|r| matches!(r, Role::Athlete)) {
        out.push("Athlete".to_string());
    }
    if roles.iter().any(|r| matches!(r, Role::Trainer | Role::SuperAdmin)) {
        out.push("Trainer".to_string());
    }
    if roles.iter().any(|r| matches!(r, Role::Admin | Role::Editor | Role::SuperAdmin)) {
        out.push("Admin".to_string());
    }
    if out.is_empty() {
        out.push("Athlete".to_string());
    }
    out
}

fn parse_preview_role(raw: &str) -> Result<Role, ApiError> {
    match raw.trim() {
        "Athlete" => Ok(Role::Athlete),
        "Trainer" => Ok(Role::Trainer),
        "Admin" => Ok(Role::Admin),
        other => Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("Invalid preview_role: {other}"),
        )),
    }
}

async fn username_by_id(state: &AppState, user_id: &str) -> Option<String> {
    let mut rows = state
        .db
        .query("SELECT username FROM users WHERE id = ?1", [user_id])
        .await
        .ok()?;
    let row = rows.next().await.ok()??;
    row.get(0).ok()
}

pub async fn role_preview_context(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path(user_id): Path<String>,
) -> Result<Json<RolePreviewContextDto>, ApiError> {
    let username = username_by_id(&state, &user_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;
    let roles = user_roles_by_id(&state, &user_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;
    let role_names: Vec<String> = roles.iter().map(|r| r.to_string()).collect();
    let preview_roles = preview_roles_for_user(&roles);
    let athlete = fetch_athlete_by_user_id(&state, &user_id).await?;
    Ok(Json(RolePreviewContextDto {
        user_id: user_id.clone(),
        username,
        roles: role_names,
        preview_roles,
        athlete_id: athlete.as_ref().map(|a| a.id.clone()),
        athlete_name: athlete.as_ref().map(|a| a.full_name.clone()),
    }))
}

pub async fn role_preview_session(
    State(state): State<AppState>,
    auth: RequireSuperAdmin,
    Json(payload): Json<RolePreviewSessionRequest>,
) -> Result<StatusCode, ApiError> {
    let claims = &auth.0;
    let action = payload.action.trim().to_lowercase();
    if action != "start" && action != "end" {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "action must be start or end",
        ));
    }
    if claims.sub == payload.target_user_id {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Cannot preview your own account",
        ));
    }
    let preview_role = parse_preview_role(&payload.preview_role)?;
    let target_roles = user_roles_by_id(&state, &payload.target_user_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;
    if target_roles.contains(&Role::SuperAdmin) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Cannot preview SuperAdmin accounts",
        ));
    }
    if !preview_roles_for_user(&target_roles).contains(&preview_role.to_string()) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Target user does not have the selected preview role",
        ));
    }

    let username = username_by_id(&state, &payload.target_user_id)
        .await
        .unwrap_or_else(|| payload.target_user_id.clone());
    let details = format!(
        "preview_role={}; target_username={}; read_only=true",
        preview_role, username
    );
    let audit_action = if action == "start" {
        "role_preview_start"
    } else {
        "role_preview_end"
    };

    let conn = state.db.raw().await;
    write_audit_log(
        conn.as_ref(),
        Some(&claims.sub),
        Some("SuperAdmin"),
        "role_preview",
        audit_action,
        Some("user"),
        Some(&payload.target_user_id),
        Some(&details),
    )
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn role_preview_athlete_bundle(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path(user_id): Path<String>,
) -> Result<Json<RolePreviewAthleteBundleDto>, ApiError> {
    let athlete = fetch_athlete_by_user_id(&state, &user_id).await?;
    let Some(ref a) = athlete else {
        return Ok(Json(RolePreviewAthleteBundleDto {
            athlete: None,
            calendar_entries: vec![],
            results: vec![],
        }));
    };
    let calendar_entries = calendar_entries_for_athlete_id(&state, &a.id).await?;
    let mut rows = state
        .db
        .query(
            "SELECT id, athlete_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg, kind, location, bodyweight_kg FROM results \
             WHERE athlete_id = ?1 ORDER BY date DESC, id DESC",
            [a.id.clone()],
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
    Ok(Json(RolePreviewAthleteBundleDto {
        athlete,
        calendar_entries,
        results,
    }))
}

pub async fn role_preview_athlete_profile(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path(user_id): Path<String>,
) -> Result<Json<Athlete>, ApiError> {
    let athlete = fetch_athlete_by_user_id(&state, &user_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete profile not found for user"))?;
    Ok(Json(athlete))
}

pub async fn role_preview_calendar(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path(user_id): Path<String>,
) -> Result<Json<MyCalendarResponse>, ApiError> {
    let athlete = fetch_athlete_by_user_id(&state, &user_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete profile not found for user"))?;
    let entries = calendar_entries_for_athlete_id(&state, &athlete.id).await?;
    Ok(Json(MyCalendarResponse { entries }))
}

pub async fn role_preview_payment_status(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path(user_id): Path<String>,
    Query(q): Query<MonthQuery>,
) -> Result<Json<PaymentStatusResponse>, ApiError> {
    let athlete = fetch_athlete_by_user_id(&state, &user_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete profile not found for user"))?;
    let month = q
        .month
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m").to_string());
    Ok(Json(
        payment_status_for_athlete_id(&state, &athlete.id, &month).await?,
    ))
}

#[derive(Serialize)]
pub struct RolePreviewExerciseSubmissionDto {
    pub id: String,
    pub athlete_id: String,
    pub exercise_id: String,
    pub exercise_name: String,
    pub value: f64,
    pub unit: String,
    pub performed_at: String,
    pub notes: Option<String>,
    pub status: String,
    pub reviewed_at: Option<String>,
    pub review_note: Option<String>,
    pub created_at: String,
}

pub async fn role_preview_exercise_submissions(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path(user_id): Path<String>,
) -> Result<Json<Vec<RolePreviewExerciseSubmissionDto>>, ApiError> {
    let athlete = fetch_athlete_by_user_id(&state, &user_id)
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Athlete profile not found for user"))?;

    let mut rows = state
        .db
        .query(
            "SELECT s.id, s.athlete_id, e.id, CAST(e.name AS BLOB), s.value, s.unit, s.performed_at, s.notes,
                    s.status, s.reviewed_at, s.review_note, s.created_at
             FROM exercise_submissions s
             JOIN exercises e ON e.id = s.exercise_id
             WHERE s.athlete_id = ?1
             ORDER BY s.created_at DESC",
            [athlete.id],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let exercise_name_bytes: Vec<u8> = row.get(3).unwrap_or_default();
        let exercise_name = String::from_utf8_lossy(&exercise_name_bytes).into_owned();
        out.push(RolePreviewExerciseSubmissionDto {
            id: row.get(0).unwrap_or_default(),
            athlete_id: row.get(1).unwrap_or_default(),
            exercise_id: row.get(2).unwrap_or_default(),
            exercise_name,
            value: row.get(4).unwrap_or(0.0),
            unit: row.get(5).unwrap_or_default(),
            performed_at: row.get(6).unwrap_or_default(),
            notes: row.get(7).ok(),
            status: row.get(8).unwrap_or_default(),
            reviewed_at: row.get(9).ok(),
            review_note: row.get(10).ok(),
            created_at: row.get(11).unwrap_or_default(),
        });
    }
    Ok(Json(out))
}

pub async fn role_preview_notifications(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path(user_id): Path<String>,
) -> Result<Json<Vec<NotificationDto>>, ApiError> {
    let _ = username_by_id(&state, &user_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;
    let conn_arc = state.db.raw().await;
    let list = repos::notifications::list_for_user(conn_arc.as_ref(), &user_id)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(list))
}

pub async fn role_preview_chat_threads(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path(user_id): Path<String>,
) -> Result<Json<Vec<ChatThreadDto>>, ApiError> {
    let _ = username_by_id(&state, &user_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;
    Ok(Json(threads_for_user(&state, &user_id).await?))
}

pub async fn role_preview_chat_messages(
    State(state): State<AppState>,
    _auth: RequireSuperAdmin,
    Path((user_id, thread_id)): Path<(String, String)>,
    Query(pagination): Query<ListPaginationQuery>,
) -> Result<Json<Vec<ChatMessageDto>>, ApiError> {
    let _ = username_by_id(&state, &user_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "User not found"))?;
    Ok(Json(
        messages_for_user(&state, &user_id, &thread_id, &pagination, false).await?,
    ))
}

