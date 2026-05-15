use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::Claims;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
};

pub async fn export_competition_ics(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _claims: Claims,
) -> Result<impl IntoResponse, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT title, category, date, location FROM competitions WHERE id = ?1",
            [id.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Competition not found"))?;

    let title: String = row.get(0).unwrap_or_default();
    let category: String = row.get(1).unwrap_or_else(|_| "Zawody".to_string());
    let date_str: String = row.get(2).unwrap_or_default();
    let location: String = row.get(3).unwrap_or_default();

    // Simple ICS generation
    // DTSTART: YYYYMMDDTHHMMSSZ (UTC)
    // For simplicity, we assume the date in DB is YYYY-MM-DD and set it to 08:00
    let date_clean = date_str.replace("-", "");
    let dtstart = format!("{}T080000Z", date_clean);
    let dtend = format!("{}T180000Z", date_clean);

    let ics = format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//Slavia//NONSGML v1.0//PL\r\n\
         BEGIN:VEVENT\r\n\
         UID:{}@slavia-club.pl\r\n\
         DTSTAMP:{}T120000Z\r\n\
         DTSTART:{}\r\n\
         DTEND:{}\r\n\
         SUMMARY:{}\r\n\
         DESCRIPTION:Kategoria: {}\r\n\
         LOCATION:{}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR",
        id, date_clean, dtstart, dtend, title, category, location
    );

    Ok((
        [(header::CONTENT_TYPE, "text/calendar")],
        [(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}.ics\"", id))],
        ics,
    ))
}
