//! Synchronizacja zawodów z PZPC (HTML) oraz z publicznego JSON podnoszenieciezarow.pl (`/kalendarz/events?year=`).

use chrono::Datelike;
use libsql::Connection;
use regex::Regex;
use serde::Serialize;
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use axum::http::StatusCode;

#[derive(Serialize)]
pub struct SyncExternalResponse {
    pub pzpc_imported: usize,
    pub pc_imported: usize,
    pub upserts: usize,
}

struct UpsertRow {
    title: String,
    date: String,
    location: String,
    description: Option<String>,
    category: String,
    status: String,
    external_source: String,
    external_ref: String,
    external_url: Option<String>,
}

fn polish_month_year(header: &str) -> Option<(u32, i32)> {
    let s = header.trim();
    let mut parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let year: i32 = parts.pop()?.parse().ok()?;
    let month_name = parts.join(" ").to_lowercase();
    let month = match month_name.as_str() {
        "styczeń" | "stycznia" => 1,
        "luty" | "lutego" => 2,
        "marzec" | "marca" => 3,
        "kwiecień" | "kwietnia" => 4,
        "maj" | "maja" => 5,
        "czerwiec" | "czerwca" => 6,
        "lipiec" | "lipca" => 7,
        "sierpień" | "sierpnia" => 8,
        "wrzesień" | "września" => 9,
        "październik" | "października" => 10,
        "listopad" | "listopada" => 11,
        "grudzień" | "grudnia" => 12,
        _ => return None,
    };
    Some((month, year))
}

fn first_day_from_span(inner: &str) -> Option<u32> {
    let t = inner.trim();
    let head = t.split_once('-').map(|(a, _)| a).unwrap_or(t);
    head.trim().parse().ok()
}

fn ymd(year: i32, month: u32, day: u32) -> String {
    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn location_from_title(title: &str) -> String {
    if let Some((_, loc)) = title.rsplit_once(',') {
        let loc = loc.trim();
        if !loc.is_empty() {
            return loc.to_string();
        }
    }
    "Polska".to_string()
}

fn parse_pzpc(html: &str) -> Result<Vec<UpsertRow>, ApiError> {
    let item_re = Regex::new(
        r#"href='/strefa-sportowa/kalendarz-imprez/(centralne|okregowe)/(\d+)'[\s\S]*?<div class='kalendarz-item--date'>[\s\S]*?<span(?: class='small')?>([^<]*)</span>\s*([^<]*?)\s*</div>[\s\S]*?<h3>([^<]+)</h3>"#,
    )
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let marker = r#"class="kalendarz-list-title">"#;
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find(marker) {
        let after = &rest[start + marker.len()..];
        let end_title = after.find("</div>").unwrap_or(after.len());
        let title_header = after[..end_title].trim();
        let after_title = &after[end_title + 6..];
        let next_marker = after_title.find(marker).unwrap_or(after_title.len());
        let fragment = &after_title[..next_marker];
        rest = &after_title[next_marker..];

        let Some((month, year)) = polish_month_year(title_header) else {
            continue;
        };

        for cap in item_re.captures_iter(fragment) {
            let scope = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let id_num = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            let day_inner = cap.get(3).map(|m| m.as_str()).unwrap_or("");
            let h3 = cap.get(5).map(|m| m.as_str().trim()).unwrap_or("");
            if h3.is_empty() {
                continue;
            }
            let Some(day) = first_day_from_span(day_inner) else {
                continue;
            };
            let category = if scope == "centralne" {
                "championship"
            } else {
                "league"
            };
            let url = format!(
                "https://pzpc.pl/strefa-sportowa/kalendarz-imprez/{}/{}",
                scope, id_num
            );
            let external_ref = format!("pzpc:{}", id_num);
            let date = ymd(year, month, day);
            let loc = location_from_title(h3);
            let description = Some(format!("Źródło: kalendarz PZPC\n{}", url));
            out.push(UpsertRow {
                title: h3.to_string(),
                date,
                location: loc,
                description,
                category: category.to_string(),
                status: "scheduled".to_string(),
                external_source: "pzpc".to_string(),
                external_ref,
                external_url: Some(url),
            });
        }
    }

    Ok(out)
}

fn pc_category(color: &str) -> &'static str {
    match color {
        "red" => "championship",
        "blue" => "championship",
        "green" => "league",
        "yellow" => "club_event",
        _ => "club_event",
    }
}

fn parse_pc_json(body: &str) -> Result<Vec<UpsertRow>, ApiError> {
    let arr: Vec<serde_json::Value> =
        serde_json::from_str(body).map_err(|e| api_error(StatusCode::BAD_REQUEST, e.to_string()))?;
    let mut out = Vec::new();
    for v in arr {
        let Some(id) = v.get("id").and_then(|x| x.as_u64()) else {
            continue;
        };
        let title = v
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let start = v.get("start").and_then(|x| x.as_str()).unwrap_or("");
        if start.len() < 10 {
            continue;
        }
        let date = start[..10].to_string();
        let end = v.get("end").and_then(|x| x.as_str()).unwrap_or("");
        if end.starts_with('-') || end.contains("-0001") {
            continue;
        }
        let color = v.get("color").and_then(|x| x.as_str()).unwrap_or("");
        let cat = pc_category(color);
        let external_ref = format!("pc:{}", id);
        let url = Some("https://podnoszenieciezarow.pl/kalendarz".to_string());
        let description = Some(format!(
            "Źródło: kalendarz PodnoszenieCiezarow.pl\n{}",
            url.as_ref().unwrap()
        ));
        let loc = location_from_title(&title);
        out.push(UpsertRow {
            title,
            date,
            location: loc,
            description,
            category: cat.to_string(),
            status: "scheduled".to_string(),
            external_source: "podnoszenieciezarow".to_string(),
            external_ref,
            external_url: url,
        });
    }
    Ok(out)
}

async fn upsert_external_row(conn: &Connection, row: UpsertRow) -> Result<(), libsql::Error> {
    let mut existing = conn
        .query(
            "SELECT id FROM competitions WHERE external_source = ?1 AND external_ref = ?2",
            (row.external_source.clone(), row.external_ref.clone()),
        )
        .await?;

    if let Some(r) = existing.next().await? {
        let id: String = r.get(0)?;
        conn.execute(
            "UPDATE competitions SET title = ?1, date = ?2, location = ?3, description = ?4, category = ?5, status = ?6, external_url = ?7 WHERE id = ?8",
            (
                row.title.clone(),
                row.date.clone(),
                row.location.clone(),
                row.description.clone(),
                row.category.clone(),
                row.status.clone(),
                row.external_url.clone(),
                id,
            ),
        )
        .await?;
    } else {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO competitions (id, title, date, location, description, category, status, external_source, external_ref, external_url) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                id,
                row.title,
                row.date,
                row.location,
                row.description,
                row.category,
                row.status,
                row.external_source,
                row.external_ref,
                row.external_url,
            ),
        )
        .await?;
    }
    Ok(())
}

pub async fn run_sync(conn: &Connection) -> Result<SyncExternalResponse, ApiError> {
    let client = reqwest::Client::builder()
        .user_agent("SlaviaClubCalendarSync/1.0 (+https://github.com)")
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let pzpc_html = client
        .get("https://pzpc.pl/strefa-sportowa/kalendarz-imprez")
        .send()
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, format!("PZPC: {}", e)))?
        .text()
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, format!("PZPC body: {}", e)))?;

    let pzpc_rows = parse_pzpc(&pzpc_html)?;

    let year_now = chrono::Utc::now().date_naive().year();
    let years = [year_now - 1, year_now, year_now + 1];
    let mut pc_rows = Vec::new();
    for y in years {
        let url = format!("https://podnoszenieciezarow.pl/kalendarz/events?year={}", y);
        let body = client
            .get(&url)
            .send()
            .await
            .map_err(|e| api_error(StatusCode::BAD_GATEWAY, format!("PC {}: {}", y, e)))?
            .text()
            .await
            .map_err(|e| api_error(StatusCode::BAD_GATEWAY, format!("PC body {}: {}", y, e)))?;
        let mut chunk = parse_pc_json(&body)?;
        pc_rows.append(&mut chunk);
    }

    let pzpc_n = pzpc_rows.len();
    let pc_n = pc_rows.len();
    let mut upserts = 0usize;

    for r in pzpc_rows {
        upsert_external_row(conn, r)
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        upserts += 1;
    }
    for r in pc_rows {
        upsert_external_row(conn, r)
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        upserts += 1;
    }

    Ok(SyncExternalResponse {
        pzpc_imported: pzpc_n,
        pc_imported: pc_n,
        upserts,
    })
}
