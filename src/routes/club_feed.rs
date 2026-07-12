use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;

use crate::api_error::{ApiError, api_error};
use crate::state::AppState;

#[derive(Debug, Serialize, Clone)]
pub struct ClubFeedItem {
    pub id: String,
    /// `post` | `announcement` | `event`
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub at: String,
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

fn plain_summary(raw: &str, max_chars: usize) -> String {
    let mut s = raw
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    if s.contains('<') {
        s = s
            .split('<')
            .enumerate()
            .filter_map(|(i, part)| {
                if i == 0 {
                    return Some(part.to_string());
                }
                part.split_once('>').map(|(_, rest)| rest.to_string())
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = collapsed.chars().count();
    if count <= max_chars {
        return collapsed;
    }
    let t: String = collapsed.chars().take(max_chars).collect();
    format!("{t}…")
}

/// Publiczny strumień: aktualności + ogłoszenia + wydarzenia z kalendarza (jeden JSON).
pub async fn list_club_feed(
    State(state): State<AppState>,
) -> Result<Json<Vec<ClubFeedItem>>, ApiError> {
    let conn_arc = state.db_conn().await?;
    let conn = conn_arc.as_ref();
    let mut items: Vec<ClubFeedItem> = Vec::new();

    let mut posts = conn
        .query(
            "SELECT id, title, content, created_at FROM posts \
             WHERE published = 1 ORDER BY created_at DESC LIMIT 25",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = posts
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id: String = row.get(0).unwrap_or_default();
        let title: String = row.get(1).unwrap_or_default();
        let content: String = row.get(2).unwrap_or_default();
        let at: String = row.get(3).unwrap_or_default();
        items.push(ClubFeedItem {
            id: id.clone(),
            kind: "post".to_string(),
            title,
            summary: plain_summary(&content, 200),
            at,
            pinned: false,
            category: None,
            location: None,
            status: None,
        });
    }

    let mut anns = conn
        .query(
            "SELECT id, title, body, pinned, created_at FROM announcements \
             WHERE published = 1 ORDER BY pinned DESC, sort_order ASC, created_at DESC LIMIT 25",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = anns
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id: String = row.get(0).unwrap_or_default();
        let title: String = row.get(1).unwrap_or_default();
        let body: String = row.get(2).unwrap_or_default();
        let pinned: i64 = row.get(3).unwrap_or(0);
        let at: String = row.get(4).unwrap_or_default();
        items.push(ClubFeedItem {
            id: id.clone(),
            kind: "announcement".to_string(),
            title,
            summary: plain_summary(&body, 200),
            at,
            pinned: pinned != 0,
            category: None,
            location: None,
            status: None,
        });
    }

    let mut events = conn
        .query(
            "SELECT id, title, date, location, category, status FROM competitions \
             WHERE (status IS NULL OR status != 'cancelled') \
             ORDER BY date DESC LIMIT 40",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    while let Some(row) = events
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let id: String = row.get(0).unwrap_or_default();
        let title: String = row.get(1).unwrap_or_default();
        let date: String = row.get(2).unwrap_or_default();
        let location: String = row.get(3).unwrap_or_default();
        let category: Option<String> = row.get(4).ok();
        let status: Option<String> = row.get(5).ok();
        let cat = category.clone().unwrap_or_default();
        let summary = if location.trim().is_empty() {
            format!("Data: {date}")
        } else {
            format!("{date} · {location}")
        };
        items.push(ClubFeedItem {
            id: id.clone(),
            kind: "event".to_string(),
            title,
            summary,
            at: date,
            pinned: false,
            category: if cat.is_empty() { None } else { Some(cat) },
            location: if location.is_empty() {
                None
            } else {
                Some(location)
            },
            status,
        });
    }

    items.sort_by(|a, b| {
        let pin = b.pinned.cmp(&a.pinned);
        if pin != std::cmp::Ordering::Equal {
            return pin;
        }
        b.at.cmp(&a.at)
    });
    items.truncate(60);

    Ok(Json(items))
}
