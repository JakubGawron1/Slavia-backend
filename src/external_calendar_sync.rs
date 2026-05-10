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
    /// Wszystkie usunięte wydarzenia z `competitions` poza oknem roku [bieżący, następny] (import + ręczne).
    pub stale_removed: u64,
    /// Z powyższego — wiersze z niepustym `external_source`.
    pub stale_import_removed: u64,
    /// Z powyższego — wpisy dodane ręcznie w klubie.
    pub stale_manual_removed: u64,
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

/// Import zewnętrzny: tylko bieżący rok kalendarzowy i następny (np. zawody już zapowiedziane).
fn event_year_in_current_or_next(date_yyyy_mm_dd: &str, year_now: i32) -> bool {
    let y: i32 = match date_yyyy_mm_dd.get(0..4).and_then(|s| s.parse().ok()) {
        Some(v) => v,
        None => return false,
    };
    y == year_now || y == year_now + 1
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

/// Heurystyka po tytule zawodów (PL) — gdy zewnętrzne API nie podaje koloru / typu.
fn infer_category_from_title(title: &str) -> Option<&'static str> {
    let t = title.to_lowercase();
    let is_champ_word = t.contains("mistrzostw")
        || t.contains("mistrzów")
        || t.contains("mistrz.")
        || t.contains("(mp)")
        || t.contains(" mp ")
        || t.ends_with(" mp")
        || t.contains(" mp,")
        || t.contains("mpp")
        || (t.contains("mistrzyni") && (t.contains("polsk") || t.contains("ślą") || t.contains("slask")));
    if is_champ_word {
        return Some("championship");
    }
    if t.contains("puchar") {
        return Some("championship");
    }
    let league_hint = t.contains(" ligi ")
        || t.contains("ligi ")
        || t.contains("ligi,")
        || t.contains("ligi.")
        || t.contains("etap ligi")
        || t.contains("baraż")
        || t.contains("baraz")
        || t.contains("eliminacj") && t.contains("lig");
    if league_hint {
        return Some("league");
    }
    None
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
            let scope_default = if scope == "centralne" {
                "championship"
            } else {
                "league"
            };
            let category = infer_category_from_title(h3).unwrap_or(scope_default);
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

fn pc_category(color: &str, title: &str) -> &'static str {
    let c = color.trim().to_lowercase();
    let from_palette = match c.as_str() {
        "red" => Some("championship"),
        "blue" => Some("championship"),
        "green" => Some("league"),
        "yellow" => Some("club_event"),
        "orange" => Some("league"),
        "purple" | "violet" => Some("championship"),
        "cyan" | "teal" => Some("league"),
        "pink" | "magenta" => Some("championship"),
        "" => None,
        _ => None,
    };
    if let Some(cat) = from_palette {
        return cat;
    }
    infer_category_from_title(title).unwrap_or("club_event")
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
        let cat = pc_category(color, &title);
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

/// Predykat SQL: data poza oknem [year_now, year_max] lub niepoprawny format `YYYY-MM-DD`.
fn stale_competition_sql_predicate() -> &'static str {
    "LENGTH(date) < 10 \
       OR CAST(SUBSTR(date, 1, 4) AS INTEGER) < ?1 \
       OR CAST(SUBSTR(date, 1, 4) AS INTEGER) > ?2"
}

async fn count_stale_competitions_import(
    conn: &Connection,
    year_now: i32,
    year_max: i32,
) -> Result<u64, libsql::Error> {
    let mut rows = conn
        .query(
            &format!(
                "SELECT COUNT(*) FROM competitions WHERE ({}) \
                   AND external_source IS NOT NULL AND TRIM(COALESCE(external_source, '')) != ''",
                stale_competition_sql_predicate()
            ),
            (year_now, year_max),
        )
        .await?;
    let Some(r) = rows.next().await? else {
        return Ok(0);
    };
    let n: i64 = r.get(0)?;
    Ok(n as u64)
}

async fn count_stale_competitions_manual(
    conn: &Connection,
    year_now: i32,
    year_max: i32,
) -> Result<u64, libsql::Error> {
    let mut rows = conn
        .query(
            &format!(
                "SELECT COUNT(*) FROM competitions WHERE ({}) \
                   AND (external_source IS NULL OR TRIM(COALESCE(external_source, '')) = '')",
                stale_competition_sql_predicate()
            ),
            (year_now, year_max),
        )
        .await?;
    let Some(r) = rows.next().await? else {
        return Ok(0);
    };
    let n: i64 = r.get(0)?;
    Ok(n as u64)
}

/// Usuwa wyniki powiązane z przeterminowanymi zawodami, potem same wiersze `competitions`
/// (`competition_participants` — CASCADE). Dotyczy **wszystkich** wpisów kalendarza.
async fn purge_stale_calendar_competitions(
    conn: &Connection,
    year_now: i32,
) -> Result<(u64, u64, u64), libsql::Error> {
    let year_max = year_now + 1;
    let stale_import = count_stale_competitions_import(conn, year_now, year_max).await?;
    let stale_manual = count_stale_competitions_manual(conn, year_now, year_max).await?;

    let pred = stale_competition_sql_predicate();
    conn.execute(
        &format!(
            "DELETE FROM results WHERE competition_id IS NOT NULL \
               AND competition_id IN (SELECT id FROM competitions WHERE ({pred}))"
        ),
        (year_now, year_max),
    )
    .await?;

    let deleted = conn
        .execute(
            &format!("DELETE FROM competitions WHERE ({pred})"),
            (year_now, year_max),
        )
        .await?;

    Ok((deleted, stale_import, stale_manual))
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
            "UPDATE competitions SET \
                title = ?1, \
                date = ?2, \
                location = ?3, \
                description = ?4, \
                category = CASE WHEN category_override IS NOT NULL THEN category ELSE ?5 END, \
                status = ?6, \
                external_url = ?7 \
             WHERE id = ?8",
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

    let year_now = chrono::Utc::now().date_naive().year();

    let pzpc_rows: Vec<UpsertRow> = parse_pzpc(&pzpc_html)?
        .into_iter()
        .filter(|r| event_year_in_current_or_next(&r.date, year_now))
        .collect();

    // Tylko rok bieżący i następny — bez ubiegłych kalendarzy z PC JSON.
    let years = [year_now, year_now + 1];
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
        let chunk = parse_pc_json(&body)?;
        pc_rows.extend(
            chunk
                .into_iter()
                .filter(|r| event_year_in_current_or_next(&r.date, year_now)),
        );
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

    let (stale_removed, stale_import_removed, stale_manual_removed) =
        purge_stale_calendar_competitions(conn, year_now)
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(SyncExternalResponse {
        pzpc_imported: pzpc_n,
        pc_imported: pc_n,
        upserts,
        stale_removed,
        stale_import_removed,
        stale_manual_removed,
    })
}
