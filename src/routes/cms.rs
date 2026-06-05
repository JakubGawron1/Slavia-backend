//! CMS — zmienne, strony, nawigacja paneli, wersjonowanie.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::Utc;
use libsql::Row;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::cms_sanitize::sanitize_cms_html;
use crate::middleware::auth::{RequireEditorOrHigher};
use crate::models::{CmsNavigationItem, CmsPage, CmsVariable, CmsVariableType, CmsVersionEntry};
use crate::sql_row;
use crate::state::AppState;

fn row_to_variable(row: &Row) -> Result<CmsVariable, libsql::Error> {
    let value_type_str = sql_row::flex_string(row, 3)?;
    let value_type = value_type_str
        .parse::<CmsVariableType>()
        .unwrap_or(CmsVariableType::Text);
    let value_raw = sql_row::flex_string(row, 2)?;
    let value: Value = serde_json::from_str(&value_raw).unwrap_or(Value::String(value_raw));
    Ok(CmsVariable {
        id: sql_row::flex_string(row, 0)?,
        key: sql_row::flex_string(row, 1)?,
        value,
        value_type,
        created_at: sql_row::flex_string(row, 4)?,
        updated_at: sql_row::flex_string(row, 5)?,
    })
}

fn row_to_page(row: &Row) -> Result<CmsPage, libsql::Error> {
    let fields_raw = sql_row::flex_string(row, 2)?;
    let fields: Value = serde_json::from_str(&fields_raw).unwrap_or(Value::Object(Default::default()));
    Ok(CmsPage {
        id: sql_row::flex_string(row, 0)?,
        page_name: sql_row::flex_string(row, 1)?,
        fields,
        created_at: sql_row::flex_string(row, 3)?,
        updated_at: sql_row::flex_string(row, 4)?,
    })
}

fn row_to_nav(row: &Row) -> Result<CmsNavigationItem, libsql::Error> {
    Ok(CmsNavigationItem {
        id: sql_row::flex_string(row, 0)?,
        role: sql_row::flex_string(row, 1)?,
        label: sql_row::flex_string(row, 2)?,
        icon: sql_row::flex_string(row, 3)?,
        url: sql_row::flex_string(row, 4)?,
        order_index: sql_row::opt_i64(row, 5)?.unwrap_or(0),
        group_name: sql_row::opt_string(row, 6)?,
        created_at: sql_row::flex_string(row, 7)?,
        updated_at: sql_row::flex_string(row, 8)?,
    })
}

fn row_to_version(row: &Row) -> Result<CmsVersionEntry, libsql::Error> {
    Ok(CmsVersionEntry {
        id: sql_row::flex_string(row, 0)?,
        entity_type: sql_row::flex_string(row, 1)?,
        entity_key: sql_row::flex_string(row, 2)?,
        old_value: sql_row::opt_string(row, 3)?,
        new_value: sql_row::opt_string(row, 4)?,
        changed_by: sql_row::opt_string(row, 5)?,
        created_at: sql_row::flex_string(row, 6)?,
    })
}

fn validate_variable_key(key: &str) -> Result<(), ApiError> {
    if key.is_empty() || key.len() > 120 {
        return Err(api_error(StatusCode::BAD_REQUEST, "key must be 1–120 characters"));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "key may only contain letters, digits, underscore and hyphen",
        ));
    }
    Ok(())
}

fn sanitize_variable_value(value: &Value, value_type: &CmsVariableType) -> Result<Value, ApiError> {
    match value_type {
        CmsVariableType::Html => {
            let s = value.as_str().unwrap_or("").to_string();
            Ok(Value::String(sanitize_cms_html(&s)))
        }
        CmsVariableType::Text | CmsVariableType::Image => {
            let s = value.as_str().unwrap_or("").trim().to_string();
            Ok(Value::String(s))
        }
        CmsVariableType::Number => {
            if value.is_number() {
                Ok(value.clone())
            } else if let Some(s) = value.as_str() {
                s.parse::<f64>()
                    .map(|n| Value::Number(serde_json::Number::from_f64(n).unwrap_or(0.into())))
                    .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid number value"))
            } else {
                Err(api_error(StatusCode::BAD_REQUEST, "invalid number value"))
            }
        }
        CmsVariableType::Boolean => {
            if value.is_boolean() {
                Ok(value.clone())
            } else if let Some(s) = value.as_str() {
                Ok(Value::Bool(matches!(s.to_lowercase().as_str(), "true" | "1" | "yes")))
            } else {
                Err(api_error(StatusCode::BAD_REQUEST, "invalid boolean value"))
            }
        }
    }
}

async fn record_version(
    state: &AppState,
    entity_type: &str,
    entity_key: &str,
    old_value: Option<&str>,
    new_value: Option<&str>,
    changed_by: &str,
) -> Result<(), ApiError> {
    state
        .db
        .execute(
            "INSERT INTO cms_version_history (id, entity_type, entity_key, old_value, new_value, changed_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                Uuid::new_v4().to_string(),
                entity_type.to_string(),
                entity_key.to_string(),
                old_value.map(|s| s.to_string()),
                new_value.map(|s| s.to_string()),
                changed_by.to_string(),
                Utc::now().to_rfc3339(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

/// Podmienia `{nazwa_zmiennej}` w tekście wartościami z mapy.
pub fn interpolate_variables(content: &str, vars: &[(String, String)]) -> String {
    let mut out = content.to_string();
    for (key, val) in vars {
        let placeholder = format!("{{{}}}", key);
        out = out.replace(&placeholder, val);
    }
    out
}

// —— Variables ——

pub async fn list_variables(
    State(state): State<AppState>,
) -> Result<Json<Vec<CmsVariable>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, key, value, value_type, created_at, updated_at FROM cms_variables ORDER BY key ASC",
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
        out.push(
            row_to_variable(&row)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct CreateVariableRequest {
    pub key: String,
    pub value: Value,
    #[serde(rename = "type", default)]
    pub value_type: Option<String>,
}

pub async fn create_variable(
    State(state): State<AppState>,
    auth: RequireEditorOrHigher,
    Json(body): Json<CreateVariableRequest>,
) -> Result<Json<CmsVariable>, ApiError> {
    validate_variable_key(&body.key)?;
    let value_type = body
        .value_type
        .as_deref()
        .unwrap_or("text")
        .parse::<CmsVariableType>()
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    let sanitized = sanitize_variable_value(&body.value, &value_type)?;
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let value_json = serde_json::to_string(&sanitized)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .db
        .execute(
            "INSERT INTO cms_variables (id, key, value, value_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id.clone(),
                body.key.clone(),
                value_json.clone(),
                value_type.to_string(),
                now.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                api_error(StatusCode::BAD_REQUEST, "variable key already exists")
            } else {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    record_version(
        &state,
        "variable",
        &body.key,
        None,
        Some(&value_json),
        &auth.0.sub,
    )
    .await?;

    let conn = state.db.raw().await;
    let _ = write_audit_log(
        &conn,
        Some(&auth.0.sub),
        Some("cms"),
        "cms",
        "create_variable",
        Some("variable"),
        Some(&body.key),
        None,
    )
    .await;

    Ok(Json(CmsVariable {
        id,
        key: body.key,
        value: sanitized,
        value_type,
        created_at: now.clone(),
        updated_at: now,
    }))
}

#[derive(Deserialize)]
pub struct UpdateVariableRequest {
    pub value: Value,
    #[serde(rename = "type", default)]
    pub value_type: Option<String>,
}

pub async fn update_variable(
    State(state): State<AppState>,
    auth: RequireEditorOrHigher,
    Path(key): Path<String>,
    Json(body): Json<UpdateVariableRequest>,
) -> Result<Json<CmsVariable>, ApiError> {
    validate_variable_key(&key)?;

    let mut rows = state
        .db
        .query(
            "SELECT id, key, value, value_type, created_at, updated_at FROM cms_variables WHERE key = ?1 LIMIT 1",
            [key.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Variable not found"))?;

    let existing = row_to_variable(&row)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let value_type = body
        .value_type
        .as_deref()
        .map(|s| s.parse::<CmsVariableType>())
        .transpose()
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?
        .unwrap_or(existing.value_type);

    let sanitized = sanitize_variable_value(&body.value, &value_type)?;
    let now = Utc::now().to_rfc3339();
    let old_json = serde_json::to_string(&existing.value).unwrap_or_default();
    let new_json = serde_json::to_string(&sanitized)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .db
        .execute(
            "UPDATE cms_variables SET value = ?1, value_type = ?2, updated_at = ?3 WHERE key = ?4",
            (new_json.clone(), value_type.to_string(), now.clone(), key.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    record_version(
        &state,
        "variable",
        &key,
        Some(&old_json),
        Some(&new_json),
        &auth.0.sub,
    )
    .await?;

    Ok(Json(CmsVariable {
        id: existing.id,
        key,
        value: sanitized,
        value_type,
        created_at: existing.created_at,
        updated_at: now,
    }))
}

pub async fn delete_variable(
    State(state): State<AppState>,
    auth: RequireEditorOrHigher,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT value FROM cms_variables WHERE key = ?1 LIMIT 1",
            [key.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let old = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| sql_row::flex_string(&r, 0).ok());

    state
        .db
        .execute("DELETE FROM cms_variables WHERE key = ?1", [key.clone()])
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(old_val) = old.as_deref() {
        record_version(&state, "variable", &key, Some(old_val), None, &auth.0.sub).await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

// —— Pages ——

pub async fn get_page(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<CmsPage>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, page_name, fields, created_at, updated_at FROM cms_pages WHERE page_name = ?1 LIMIT 1",
            [name.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Ok(Json(
            row_to_page(&row)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        ));
    }

    let now = Utc::now().to_rfc3339();
    Ok(Json(CmsPage {
        id: String::new(),
        page_name: name,
        fields: Value::Object(Default::default()),
        created_at: now.clone(),
        updated_at: now,
    }))
}

#[derive(Deserialize)]
pub struct UpdatePageRequest {
    pub fields: Value,
}

pub async fn update_page(
    State(state): State<AppState>,
    auth: RequireEditorOrHigher,
    Path(name): Path<String>,
    Json(body): Json<UpdatePageRequest>,
) -> Result<Json<CmsPage>, ApiError> {
    if name.is_empty() || name.len() > 120 {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid page name"));
    }

    let fields = sanitize_page_fields(&body.fields)?;
    let fields_json = serde_json::to_string(&fields)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let now = Utc::now().to_rfc3339();

    let mut rows = state
        .db
        .query(
            "SELECT id, page_name, fields, created_at, updated_at FROM cms_pages WHERE page_name = ?1 LIMIT 1",
            [name.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let existing = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (id, created_at, old_json) = if let Some(row) = existing {
        let page = row_to_page(&row)
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        (
            page.id,
            page.created_at,
            serde_json::to_string(&page.fields).ok(),
        )
    } else {
        (Uuid::new_v4().to_string(), now.clone(), None)
    };

    if old_json.is_some() {
        state
            .db
            .execute(
                "UPDATE cms_pages SET fields = ?1, updated_at = ?2 WHERE page_name = ?3",
                (fields_json.clone(), now.clone(), name.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else {
        state
            .db
            .execute(
                "INSERT INTO cms_pages (id, page_name, fields, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (id.clone(), name.clone(), fields_json.clone(), created_at.clone(), now.clone()),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    record_version(
        &state,
        "page",
        &name,
        old_json.as_deref(),
        Some(&fields_json),
        &auth.0.sub,
    )
    .await?;

    Ok(Json(CmsPage {
        id,
        page_name: name,
        fields,
        created_at,
        updated_at: now,
    }))
}

fn sanitize_page_fields(fields: &Value) -> Result<Value, ApiError> {
    let Some(obj) = fields.as_object() else {
        return Err(api_error(StatusCode::BAD_REQUEST, "fields must be an object"));
    };
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        if let Some(field_obj) = v.as_object() {
            let mut field = field_obj.clone();
            let field_type = field
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("text");
            if let Some(val) = field.get("value") {
                let sanitized = if field_type == "html" {
                    Value::String(sanitize_cms_html(val.as_str().unwrap_or("")))
                } else {
                    val.clone()
                };
                field.insert("value".to_string(), sanitized);
            }
            out.insert(k.clone(), Value::Object(field));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Value::Object(out))
}

pub async fn list_pages(
    State(state): State<AppState>,
) -> Result<Json<Vec<CmsPage>>, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT id, page_name, fields, created_at, updated_at FROM cms_pages ORDER BY page_name ASC",
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
        out.push(
            row_to_page(&row)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
    }
    Ok(Json(out))
}

// —— Navigation ——

#[derive(Deserialize)]
pub struct NavigationQuery {
    pub role: Option<String>,
}

pub async fn list_navigation(
    State(state): State<AppState>,
    Query(q): Query<NavigationQuery>,
) -> Result<Json<Vec<CmsNavigationItem>>, ApiError> {
    let mut out = Vec::new();
    if let Some(role) = q.role.filter(|r| !r.is_empty()) {
        let mut rows = state
            .db
            .query(
                "SELECT id, role, label, icon, url, order_index, group_name, created_at, updated_at
                 FROM cms_navigation_items WHERE role = ?1 ORDER BY order_index ASC",
                [role],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            out.push(
                row_to_nav(&row)
                    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            );
        }
    } else {
        let mut rows = state
            .db
            .query(
                "SELECT id, role, label, icon, url, order_index, group_name, created_at, updated_at
                 FROM cms_navigation_items ORDER BY role ASC, order_index ASC",
                (),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            out.push(
                row_to_nav(&row)
                    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            );
        }
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct NavigationItemInput {
    pub id: Option<String>,
    pub role: String,
    pub label: String,
    pub icon: String,
    pub url: String,
    pub order_index: i64,
    #[serde(default)]
    pub group_name: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateNavigationRequest {
    pub items: Vec<NavigationItemInput>,
}

pub async fn update_navigation(
    State(state): State<AppState>,
    auth: RequireEditorOrHigher,
    Json(body): Json<UpdateNavigationRequest>,
) -> Result<Json<Vec<CmsNavigationItem>>, ApiError> {
    let now = Utc::now().to_rfc3339();
    let mut result = Vec::new();

    let mut old_rows = state
        .db
        .query(
            "SELECT id, role, label, icon, url, order_index, group_name, created_at, updated_at
             FROM cms_navigation_items ORDER BY role ASC, order_index ASC",
            (),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut old_items = Vec::new();
    while let Some(row) = old_rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        old_items.push(
            row_to_nav(&row)
                .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        );
    }
    let old_json = serde_json::to_string(&old_items).unwrap_or_default();

    state
        .db
        .execute("DELETE FROM cms_navigation_items", ())
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    for item in &body.items {
        let id = item
            .id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        state
            .db
            .execute(
                "INSERT INTO cms_navigation_items (id, role, label, icon, url, order_index, group_name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                (
                    id.clone(),
                    item.role.clone(),
                    item.label.clone(),
                    item.icon.clone(),
                    item.url.clone(),
                    item.order_index,
                    item.group_name.clone(),
                    now.clone(),
                    now.clone(),
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        result.push(CmsNavigationItem {
            id,
            role: item.role.clone(),
            label: item.label.clone(),
            icon: item.icon.clone(),
            url: item.url.clone(),
            order_index: item.order_index,
            group_name: item.group_name.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        });
    }

    let new_json = serde_json::to_string(&result).unwrap_or_default();
    record_version(
        &state,
        "navigation",
        "all",
        Some(&old_json),
        Some(&new_json),
        &auth.0.sub,
    )
    .await?;

    Ok(Json(result))
}

// —— Version history ——

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub entity_type: Option<String>,
    pub entity_key: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_version_history(
    State(state): State<AppState>,
    auth: RequireEditorOrHigher,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<CmsVersionEntry>>, ApiError> {
    let _ = auth;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let mut out = Vec::new();
    if let (Some(et), Some(ek)) = (q.entity_type, q.entity_key) {
        let mut rows = state
            .db
            .query(
                "SELECT id, entity_type, entity_key, old_value, new_value, changed_by, created_at
                 FROM cms_version_history WHERE entity_type = ?1 AND entity_key = ?2
                 ORDER BY created_at DESC LIMIT ?3",
                (et, ek, limit),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            out.push(
                row_to_version(&row)
                    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            );
        }
    } else {
        let mut rows = state
            .db
            .query(
                "SELECT id, entity_type, entity_key, old_value, new_value, changed_by, created_at
                 FROM cms_version_history ORDER BY created_at DESC LIMIT ?1",
                [limit],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            out.push(
                row_to_version(&row)
                    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            );
        }
    }
    Ok(Json(out))
}
