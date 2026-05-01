//! Bezpieczne odczyty z libsql — `Row::get::<f64>()` panikuje przy `Integer` (SQLite często tak zwraca REAL).

use libsql::{Row, Value};

pub fn opt_f64(row: &Row, idx: i32) -> Result<Option<f64>, libsql::Error> {
    Ok(match row.get_value(idx)? {
        Value::Null => None,
        Value::Real(f) => Some(f),
        Value::Integer(i) => Some(i as f64),
        _ => None,
    })
}

pub fn opt_i64(row: &Row, idx: i32) -> Result<Option<i64>, libsql::Error> {
    Ok(match row.get_value(idx)? {
        Value::Null => None,
        Value::Integer(i) => Some(i),
        Value::Real(f) => Some(f as i64),
        _ => None,
    })
}

pub fn opt_string(row: &Row, idx: i32) -> Result<Option<String>, libsql::Error> {
    Ok(match row.get_value(idx)? {
        Value::Null => None,
        Value::Text(s) => Some(s),
        _ => None,
    })
}

pub fn string(row: &Row, idx: i32) -> Result<String, libsql::Error> {
    Ok(opt_string(row, idx)?.unwrap_or_default())
}

pub fn bool_active(row: &Row, idx: i32) -> Result<bool, libsql::Error> {
    Ok(match row.get_value(idx)? {
        Value::Null => true,
        Value::Integer(i) => i != 0,
        Value::Real(f) => f != 0.0,
        _ => true,
    })
}

pub fn required_f64(row: &Row, idx: i32) -> Result<f64, String> {
    opt_f64(row, idx)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("brak wartości REAL/INTEGER w kolumnie {}", idx))
}

pub fn required_string(row: &Row, idx: i32) -> Result<String, String> {
    opt_string(row, idx)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("brak TEXT w kolumnie {}", idx))
}

/// Tekst z kolumny bez paniki: SQLite często zwraca INTEGER zamiast TEXT (`created_at`, legacy dane).
pub fn flex_string(row: &Row, idx: i32) -> Result<String, libsql::Error> {
    Ok(match row.get_value(idx)? {
        Value::Null => String::new(),
        Value::Text(s) => s,
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Blob(_) => String::new(),
    })
}

pub fn flex_opt_string(row: &Row, idx: i32) -> Result<Option<String>, libsql::Error> {
    Ok(match row.get_value(idx)? {
        Value::Null => None,
        Value::Text(s) => Some(s),
        Value::Integer(i) => Some(i.to_string()),
        Value::Real(f) => Some(f.to_string()),
        Value::Blob(_) => None,
    })
}
