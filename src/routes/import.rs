use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use chrono::Utc;
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use uuid::Uuid;

use crate::api_error::{api_error, ApiError};
use crate::middleware::auth::Claims;
use crate::models::Role;
use crate::sql_row;
use crate::state::AppState;

const MAX_REJECT_SAMPLES: usize = 36;
const PARSE_CAP_REGEX: usize = 120;

#[derive(Serialize)]
pub struct ImportRejectedSample {
    pub full_name: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct ImportResult {
    pub source: String,
    pub urls_attempted: usize,
    pub urls_fetched_ok: usize,
    pub fetch_errors: Vec<String>,
    pub rows_parsed: usize,
    pub records_matched_roster: usize,
    pub records_saved: usize,
    pub records_duplicate_skipped: usize,
    pub records_preview_importable: usize,
    #[serde(default)]
    pub rejected_samples: Vec<ImportRejectedSample>,
    /// Liczba rekordów dopasowanych do kadry klubu (jak dawniej `athletes_updated`).
    pub athletes_updated: usize,
    /// Zapisane (`dev_mode=true`) albo liczba rekordów kwalifikujących się do importu w podglądzie (`dev_mode=false`).
    pub new_results: usize,
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub dev_mode: Option<bool>,
}

pub async fn import_data_handler(
    State(_state): State<AppState>,
    claims: Claims,
    Json(_payload): Json<ImportRequest>,
) -> Result<Json<Vec<ImportResult>>, ApiError> {
    if !claims.roles.contains(&Role::SuperAdmin) {
        return Err(api_error(StatusCode::FORBIDDEN, "Only superadmin can import data"));
    }
    Err(api_error(
        StatusCode::GONE,
        "Import danych zawodników z federacji został wyłączony. Import zawodów pozostaje dostępny w kalendarzu.",
    ))
}

#[derive(Clone)]
struct ExternalResultSeed {
    full_name: String,
    snatch: f64,
    clean_and_jerk: f64,
    competition_title: String,
    external_source: String,
    external_ref: String,
}

struct SourceFetchOutcome {
    rows: Vec<ExternalResultSeed>,
    urls_attempted: usize,
    urls_fetched_ok: usize,
    fetch_errors: Vec<String>,
}

async fn import_from_pzpc(state: &AppState, dev_mode: bool) -> Result<ImportResult, ApiError> {
    let fetch = fetch_source_rows_live(
        "PZPC",
        "pzpc",
        &[
            "https://www.pzpc.pl",
            "https://www.pzpc.pl/wyniki",
            "https://www.pzpc.pl/kalendarz",
        ],
    )
    .await;
    eprintln!(
        "[import:pzpc] fetch_ok={}/{} parsed_rows={}",
        fetch.urls_fetched_ok,
        fetch.urls_attempted,
        fetch.rows.len()
    );
    run_source_import(state, "PZPC", fetch, dev_mode).await
}

async fn import_from_slaski(state: &AppState, dev_mode: bool) -> Result<ImportResult, ApiError> {
    let fetch = fetch_source_rows_live(
        "Śląski Związek",
        "slaski",
        &[
            "https://www.sztangisci.org",
            "https://www.sztangisci.org/wyniki",
            "https://www.sztangisci.org/kalendarz",
        ],
    )
    .await;
    eprintln!(
        "[import:slaski] fetch_ok={}/{} parsed_rows={}",
        fetch.urls_fetched_ok,
        fetch.urls_attempted,
        fetch.rows.len()
    );
    run_source_import(state, "Śląski Związek", fetch, dev_mode).await
}

async fn import_from_podnoszenie(state: &AppState, dev_mode: bool) -> Result<ImportResult, ApiError> {
    let fetch = fetch_source_rows_live(
        "podnoszenieciezarów.pl",
        "podnoszenieciezarow",
        &[
            "https://podnoszenieciezarow.pl",
            "https://podnoszenieciezarow.pl/wyniki",
            "https://podnoszenieciezarow.pl/ranking",
        ],
    )
    .await;
    eprintln!(
        "[import:podnoszenieciezarow] fetch_ok={}/{} parsed_rows={}",
        fetch.urls_fetched_ok,
        fetch.urls_attempted,
        fetch.rows.len()
    );
    run_source_import(state, "podnoszenieciezarów.pl", fetch, dev_mode).await
}

async fn run_source_import(
    state: &AppState,
    source_label: &str,
    fetch: SourceFetchOutcome,
    dev_mode: bool,
) -> Result<ImportResult, ApiError> {
    let rows = &fetch.rows;
    let rows_parsed = rows.len();
    let athlete_index = load_athlete_index(state).await?;

    let mut matched = 0usize;
    let mut saved = 0usize;
    let mut preview_importable = 0usize;
    let mut dup_skip = 0usize;
    let mut rejected_samples: Vec<ImportRejectedSample> = Vec::new();

    for row in rows {
        let Some(athlete_id) = resolve_athlete_id(&athlete_index, &row.full_name) else {
            if rejected_samples.len() < MAX_REJECT_SAMPLES {
                rejected_samples.push(ImportRejectedSample {
                    full_name: row.full_name.clone(),
                    reason: "Brak dopasowania do zawodnika z bazy klubu".into(),
                });
            }
            continue;
        };

        matched += 1;

        if !dev_mode {
            preview_importable += 1;
            continue;
        }

        let competition_id = ensure_competition_external(
            state,
            &row.competition_title,
            &row.external_source,
            &row.external_ref,
        )
        .await?;

        let total = row.snatch + row.clean_and_jerk;
        let date = Utc::now().date_naive().to_string();
        let mut existing = state
            .db
            .query(
                "SELECT id FROM results WHERE athlete_id = ?1 AND competition_id = ?2 LIMIT 1",
                (athlete_id.clone(), competition_id.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let already_exists = existing
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .is_some();
        if already_exists {
            dup_skip += 1;
            continue;
        }

        state
            .db
            .execute(
                "INSERT INTO results (id, athlete_id, competition_id, snatch, clean_and_jerk, total, status, date, squat_kg, bench_kg, deadlift_kg) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Approved', ?7, NULL, NULL, NULL)",
                (
                    Uuid::new_v4().to_string(),
                    athlete_id,
                    competition_id,
                    row.snatch,
                    row.clean_and_jerk,
                    total,
                    date,
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        saved += 1;
    }

    eprintln!(
        "[import:{}] dev_mode={} matched={} saved={} preview={} dup_skip={} rejected_parse_side={}",
        source_label,
        dev_mode,
        matched,
        saved,
        preview_importable,
        dup_skip,
        rows_parsed.saturating_sub(matched)
    );

    Ok(ImportResult {
        source: source_label.to_string(),
        urls_attempted: fetch.urls_attempted,
        urls_fetched_ok: fetch.urls_fetched_ok,
        fetch_errors: fetch.fetch_errors,
        rows_parsed,
        records_matched_roster: matched,
        records_saved: saved,
        records_duplicate_skipped: dup_skip,
        records_preview_importable: preview_importable,
        rejected_samples,
        athletes_updated: matched,
        new_results: if dev_mode { saved } else { preview_importable },
    })
}

async fn ensure_competition_external(
    state: &AppState,
    title: &str,
    external_source: &str,
    external_ref: &str,
) -> Result<String, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id FROM competitions WHERE external_source = ?1 AND external_ref = ?2 LIMIT 1",
            (external_source.to_string(), external_ref.to_string()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return sql_row::flex_string(&row, 0)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }

    let id = Uuid::new_v4().to_string();
    let date = Utc::now().date_naive().to_string();
    state
        .db
        .execute(
            "INSERT INTO competitions (id, title, date, location, description, category, status, external_source, external_ref) VALUES (?1, ?2, ?3, ?4, ?5, 'championship', 'scheduled', ?6, ?7)",
            (
                id.clone(),
                title.to_string(),
                date,
                "Import federacji".to_string(),
                Some("Automatyczny import danych z federacji".to_string()),
                external_source.to_string(),
                external_ref.to_string(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(id)
}

fn name_lookup_keys(full_name: &str) -> Vec<String> {
    let n = normalize_name(full_name);
    let mut set = HashSet::<String>::new();
    set.insert(n.clone());
    let parts: Vec<&str> = n.split_whitespace().collect();
    if parts.len() >= 2 {
        let last = parts[parts.len() - 1];
        let rest = parts[..parts.len() - 1].join(" ");
        set.insert(format!("{last} {rest}"));
        set.insert(format!("{} {}", parts[0], last));
        if let Some(fc) = parts[0].chars().next() {
            set.insert(format!("{fc}. {last}"));
            set.insert(format!("{fc} {last}"));
        }
    }
    set.into_iter().collect()
}

async fn load_athlete_index(state: &AppState) -> Result<HashMap<String, String>, ApiError> {
    let mut rows = state
        .db
        .query("SELECT id, full_name FROM athletes WHERE COALESCE(is_active, 1) = 1", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut map = HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id = sql_row::flex_string(&row, 0)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let full_name = sql_row::flex_string(&row, 1)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for key in name_lookup_keys(&full_name) {
            map.entry(key).or_insert_with(|| id.clone());
        }
    }
    Ok(map)
}

fn resolve_athlete_id(index: &HashMap<String, String>, display_name: &str) -> Option<String> {
    let n = normalize_name(display_name);
    if let Some(id) = index.get(&n) {
        return Some(id.clone());
    }
    let parts: Vec<&str> = n.split_whitespace().collect();
    if parts.len() >= 2 {
        let last = parts[parts.len() - 1];
        let rest = parts[..parts.len() - 1].join(" ");
        let rev = format!("{last} {rest}");
        if let Some(id) = index.get(&rev) {
            return Some(id.clone());
        }
        let fl = format!("{} {}", parts[0], last);
        if let Some(id) = index.get(&fl) {
            return Some(id.clone());
        }
        if let Some(fc) = parts[0].chars().next() {
            let dotted = format!("{fc}. {last}");
            if let Some(id) = index.get(&dotted) {
                return Some(id.clone());
            }
            let spaced = format!("{fc} {last}");
            if let Some(id) = index.get(&spaced) {
                return Some(id.clone());
            }
        }
    }
    None
}

fn normalize_name(name: &str) -> String {
    name.to_lowercase()
        .replace('ą', "a")
        .replace('ć', "c")
        .replace('ę', "e")
        .replace('ł', "l")
        .replace('ń', "n")
        .replace('ó', "o")
        .replace('ś', "s")
        .replace('ż', "z")
        .replace('ź', "z")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

async fn fetch_source_rows_live(source_label: &str, source_slug: &str, urls: &[&str]) -> SourceFetchOutcome {
    let client = match Client::builder()
        .timeout(Duration::from_secs(14))
        .user_agent("SlaviaImporter/1.1 (+https://slavia.local)")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return SourceFetchOutcome {
                rows: Vec::new(),
                urls_attempted: urls.len(),
                urls_fetched_ok: 0,
                fetch_errors: vec![format!("build_client: {e}")],
            };
        }
    };

    let attempted = urls.len();
    let mut ok_count = 0usize;
    let mut fetch_errors: Vec<String> = Vec::new();
    let mut out: Vec<ExternalResultSeed> = Vec::new();

    for url in urls {
        match client.get(*url).send().await {
            Err(e) => fetch_errors.push(format!("{url}: {e}")),
            Ok(resp) => {
                let status = resp.status();
                if !status.is_success() {
                    fetch_errors.push(format!("{url}: HTTP {}", status));
                    continue;
                }
                match resp.text().await {
                    Err(e) => fetch_errors.push(format!("{url}: odczyt treści: {e}")),
                    Ok(body) => {
                        ok_count += 1;
                        let mut parsed = parse_for_source(source_label, source_slug, url, &body);
                        if parsed.is_empty() {
                            parsed = parse_results_from_tables(source_label, source_slug, url, &body);
                        }
                        out.append(&mut parsed);
                    }
                }
            }
        }
    }

    SourceFetchOutcome {
        rows: dedup_rows(out),
        urls_attempted: attempted,
        urls_fetched_ok: ok_count,
        fetch_errors,
    }
}

fn parse_for_source(
    source_label: &str,
    source_slug: &str,
    url: &str,
    body: &str,
) -> Vec<ExternalResultSeed> {
    match source_slug {
        "pzpc" => parse_pzpc_html(source_label, source_slug, url, body),
        "slaski" => parse_slaski_html(source_label, source_slug, url, body),
        "podnoszenieciezarow" => parse_podnoszenie_html(source_label, source_slug, url, body),
        _ => parse_regex_generic(source_label, source_slug, url, body),
    }
}

fn parse_pzpc_html(source_label: &str, source_slug: &str, url: &str, body: &str) -> Vec<ExternalResultSeed> {
    parse_regex_named(source_label, source_slug, url, body, PARSE_CAP_REGEX)
}

fn parse_slaski_html(source_label: &str, source_slug: &str, url: &str, body: &str) -> Vec<ExternalResultSeed> {
    let mut v = parse_regex_slaski(source_label, source_slug, url, body, PARSE_CAP_REGEX);
    if v.is_empty() {
        v = parse_regex_named(source_label, source_slug, url, body, PARSE_CAP_REGEX);
    }
    v
}

fn parse_podnoszenie_html(source_label: &str, source_slug: &str, url: &str, body: &str) -> Vec<ExternalResultSeed> {
    let mut v = parse_regex_podnoszenie(source_label, source_slug, url, body, PARSE_CAP_REGEX);
    if v.is_empty() {
        v = parse_regex_named(source_label, source_slug, url, body, PARSE_CAP_REGEX);
    }
    v
}

/// Dwubój: `Imię Nazwisko … snatch / cj` — luźniejszy odstęp między imieniem a liczbami (strony CMS).
fn parse_regex_named(
    source_label: &str,
    source_slug: &str,
    url: &str,
    body: &str,
    cap: usize,
) -> Vec<ExternalResultSeed> {
    let Ok(re) = Regex::new(
        r"(?m)([A-ZŁŚŻŹĆŃÓ][a-ząćęłńóśźż\-]+(?:\s+[A-ZŁŚŻŹĆŃÓ][a-ząćęłńóśźż\-]+)+)[^\n\r]{0,200}?(\d{2,3}(?:[.,]\d)?)\s*(?:/|\||-|\+|\u{2014}|\u{2013})\s*(\d{2,3}(?:[.,]\d)?)",
    ) else {
        return Vec::new();
    };
    seeds_from_regex_capture(source_label, source_slug, url, body, &re, cap)
}

fn parse_regex_slaski(
    source_label: &str,
    source_slug: &str,
    url: &str,
    body: &str,
    cap: usize,
) -> Vec<ExternalResultSeed> {
    let Ok(re) = Regex::new(
        r"(?m)([A-ZŁŚŻŹĆŃÓ][a-ząćęłńóśźż\-]+(?:\s+[A-ZŁŚŻŹĆŃÓ][a-ząćęłńóśźż\-]+)+)[^\n\r]{0,220}?(\d{2,3}(?:[.,]\d)?)\s*[,;]\s*(\d{2,3}(?:[.,]\d)?)",
    ) else {
        return Vec::new();
    };
    seeds_from_regex_capture(source_label, source_slug, url, body, &re, cap)
}

fn parse_regex_podnoszenie(
    source_label: &str,
    source_slug: &str,
    url: &str,
    body: &str,
    cap: usize,
) -> Vec<ExternalResultSeed> {
    let Ok(re) = Regex::new(
        r"(?m)([A-ZŁŚŻŹĆŃÓ][a-ząćęłńóśźż\-]+(?:\s+[A-ZŁŚŻŹĆŃÓ][a-ząćęłńóśźż\-]+)+)[^\n\r]{0,200}?(\d{2,3}(?:[.,]\d)?)\s*(?:×|x|X)\s*(\d{2,3}(?:[.,]\d)?)",
    ) else {
        return Vec::new();
    };
    seeds_from_regex_capture(source_label, source_slug, url, body, &re, cap)
}

fn parse_regex_generic(
    source_label: &str,
    source_slug: &str,
    url: &str,
    body: &str,
) -> Vec<ExternalResultSeed> {
    parse_regex_named(source_label, source_slug, url, body, PARSE_CAP_REGEX)
}

fn seeds_from_regex_capture(
    source_label: &str,
    source_slug: &str,
    url: &str,
    body: &str,
    re: &Regex,
    cap: usize,
) -> Vec<ExternalResultSeed> {
    let title = format!("Import {source_label} {}", Utc::now().format("%Y-%m-%d"));
    let mut out = Vec::new();
    for caps in re.captures_iter(body).take(cap) {
        let name = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
        let sn_raw = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        let cj_raw = caps.get(3).map(|m| m.as_str()).unwrap_or_default();
        let Ok(snatch) = sn_raw.replace(',', ".").parse::<f64>() else {
            continue;
        };
        let Ok(clean_and_jerk) = cj_raw.replace(',', ".").parse::<f64>() else {
            continue;
        };
        if !(20.0..=300.0).contains(&snatch) || !(20.0..=350.0).contains(&clean_and_jerk) {
            continue;
        }
        let external_ref = build_external_ref(source_slug, url, name, snatch, clean_and_jerk);
        out.push(ExternalResultSeed {
            full_name: name.to_string(),
            snatch,
            clean_and_jerk,
            competition_title: title.clone(),
            external_source: source_slug.to_string(),
            external_ref,
        });
    }
    out
}

fn parse_results_from_tables(
    source_label: &str,
    source_slug: &str,
    url: &str,
    body: &str,
) -> Vec<ExternalResultSeed> {
    let Ok(sel_tr) = Selector::parse("tr") else {
        return Vec::new();
    };
    let doc = Html::parse_document(body);
    let Ok(re) = Regex::new(
        r"([A-ZŁŚŻŹĆŃÓ][a-ząćęłńóśźż\-]+(?:\s+[A-ZŁŚŻŹĆŃÓ][a-ząćęłńóśźż\-]+)+).*?(\d{2,3}(?:[.,]\d)?)\s*(?:/|\||-|\+|,|;|×|x|X|\u{2014}|\u{2013})\s*(\d{2,3}(?:[.,]\d)?)",
    ) else {
        return Vec::new();
    };
    let title = format!("Import {source_label} {}", Utc::now().format("%Y-%m-%d"));
    let mut out = Vec::new();
    for tr in doc.select(&sel_tr) {
        let line = tr
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if line.len() < 14 {
            continue;
        }
        let Some(caps) = re.captures(&line) else {
            continue;
        };
        let name = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
        let sn_raw = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        let cj_raw = caps.get(3).map(|m| m.as_str()).unwrap_or_default();
        let Ok(snatch) = sn_raw.replace(',', ".").parse::<f64>() else {
            continue;
        };
        let Ok(clean_and_jerk) = cj_raw.replace(',', ".").parse::<f64>() else {
            continue;
        };
        if !(20.0..=300.0).contains(&snatch) || !(20.0..=350.0).contains(&clean_and_jerk) {
            continue;
        }
        let external_ref = build_external_ref(source_slug, url, name, snatch, clean_and_jerk);
        out.push(ExternalResultSeed {
            full_name: name.to_string(),
            snatch,
            clean_and_jerk,
            competition_title: title.clone(),
            external_source: source_slug.to_string(),
            external_ref,
        });
        if out.len() >= PARSE_CAP_REGEX {
            break;
        }
    }
    out
}

fn build_external_ref(source_slug: &str, url: &str, name: &str, snatch: f64, clean_and_jerk: f64) -> String {
    let input = format!("{source_slug}|{url}|{}|{snatch:.1}|{clean_and_jerk:.1}", normalize_name(name));
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let hex = hex::encode(digest);
    format!("{source_slug}-{}", &hex[..16.min(hex.len())])
}

fn dedup_rows(rows: Vec<ExternalResultSeed>) -> Vec<ExternalResultSeed> {
    let mut seen = HashMap::<String, ()>::new();
    let mut out = Vec::new();
    for row in rows {
        if seen.contains_key(&row.external_ref) {
            continue;
        }
        seen.insert(row.external_ref.clone(), ());
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_and_alias_keys() {
        let k = name_lookup_keys("Jan Kowalski");
        assert!(k.contains(&"jan kowalski".to_string()));
        assert!(k.contains(&"kowalski jan".to_string()));
    }

    #[test]
    fn parse_named_regex_finds_row() {
        let html = r#"<html><body>
<p>Wyniki: Anna Nowak 80 / 100 w kat. 59kg</p>
</body></html>"#;
        let v = parse_regex_named("Test", "pzpc", "https://ex.test/w", html, 10);
        assert_eq!(v.len(), 1);
        assert_eq!(normalize_name(&v[0].full_name), "anna nowak");
        assert!((v[0].snatch - 80.0).abs() < f64::EPSILON);
        assert!((v[0].clean_and_jerk - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_uses_reversed_order_in_index() {
        let mut m = HashMap::new();
        for key in name_lookup_keys("Jan Kowalski") {
            m.insert(key, "id-1".into());
        }
        let id = resolve_athlete_id(&m, "Kowalski Jan");
        assert_eq!(id.as_deref(), Some("id-1"));
    }

    #[test]
    fn table_fallback_parses_tr() {
        let html =
            "<table><tr><td>Marta Wiśniewska</td><td colspan=\"2\">71 / 95</td></tr></table>";
        let v = parse_results_from_tables("T", "slaski", "https://x", html);
        assert!(!v.is_empty());
    }

    #[test]
    fn resolve_matches_initials_plus_lastname() {
        let mut m = HashMap::new();
        for key in name_lookup_keys("Jan Kowalski") {
            m.insert(key, "id-jk".into());
        }
        assert_eq!(resolve_athlete_id(&m, "J. Kowalski").as_deref(), Some("id-jk"));
        assert_eq!(resolve_athlete_id(&m, "J Kowalski").as_deref(), Some("id-jk"));
    }

    #[test]
    fn fixture_import_sample_html_parses() {
        const HTML: &str =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/import_sample.html"));
        let mut v = parse_for_source("Fixture", "pzpc", "https://fixture.test/", HTML);
        if v.len() < 2 {
            v.extend(parse_results_from_tables(
                "Fixture",
                "pzpc",
                "https://fixture.test/",
                HTML,
            ));
        }
        v = dedup_rows(v);
        assert!(v.len() >= 2, "expected ≥2 parsed seeds, got {}", v.len());
    }

    #[test]
    fn unicode_dash_separates_snatch_and_cj() {
        let html = "<p>Jan Testowy 80–100</p>";
        let v = parse_regex_named("T", "pzpc", "https://x", html, 10);
        assert_eq!(v.len(), 1);
        assert!((v[0].snatch - 80.0).abs() < f64::EPSILON);
        assert!((v[0].clean_and_jerk - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fixture_import_unicode_dash_html_parses() {
        const HTML: &str =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/import_unicode_dash.html"));
        let mut v = parse_for_source("Fixture", "pzpc", "https://fixture.test/", HTML);
        if v.len() < 2 {
            v.extend(parse_results_from_tables(
                "Fixture",
                "pzpc",
                "https://fixture.test/",
                HTML,
            ));
        }
        v = dedup_rows(v);
        assert!(v.len() >= 2, "expected ≥2 parsed seeds, got {}", v.len());
        let names: Vec<String> = v.iter().map(|r| normalize_name(&r.full_name)).collect();
        assert!(names.iter().any(|n| n.contains("barbara")), "{names:?}");
        assert!(names.iter().any(|n| n.contains("tomasz")), "{names:?}");
    }
}
