//! Wersjonowane migracje SQL — pliki w `sql/migrations/`, śledzenie w `schema_migrations`.

use chrono::Utc;
use libsql::Connection;

const MIGRATIONS: &[(&str, &str)] = &[(
    "20260616_001_performance_indexes",
    include_str!("../sql/migrations/20260616_001_performance_indexes.sql"),
)];

async fn migration_applied(conn: &Connection, version: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM schema_migrations WHERE version = ?1 LIMIT 1",
            [version.to_string()],
        )
        .await
        .map_err(|e| format!("schema_migrations lookup ({version}): {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("schema_migrations row ({version}): {e}"))?
        .is_some())
}

async fn apply_sql_statements(conn: &Connection, script: &str) -> Result<(), String> {
    let cleaned: String = script
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("--") {
                String::new()
            } else {
                trimmed.to_string()
            }
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    for stmt in cleaned.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        conn.execute(stmt, ())
            .await
            .map_err(|e| format!("migration SQL failed: {e}\n---\n{stmt}"))?;
    }
    Ok(())
}

/// Uruchamia brakujące migracje z `sql/migrations/`.
pub async fn apply_pending(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        )",
        (),
    )
    .await
    .map_err(|e| format!("schema_migrations DDL: {e}"))?;

    for (version, sql) in MIGRATIONS {
        if migration_applied(conn, version).await? {
            continue;
        }
        tracing::info!(version, "Applying SQL migration");
        apply_sql_statements(conn, sql).await?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            (version.to_string(), Utc::now().to_rfc3339()),
        )
        .await
        .map_err(|e| format!("schema_migrations insert ({version}): {e}"))?;
        tracing::info!(version, "SQL migration applied");
    }
    Ok(())
}
