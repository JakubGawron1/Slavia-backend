//! Schemat bazy — migracje **addytywne** (bezpieczne dla istniejących danych):
//! - `CREATE TABLE IF NOT EXISTS` — nowe tabele (np. `competition_participants`) bez kasowania wierszy.
//! - `ALTER TABLE ... ADD COLUMN` — nowe kolumny (`avatar_url` itd.); powtórne uruchomienie
//!   kończy się błędem „duplicate column”, który jest ignorowany (`let _ =`).
//!
//! **Nie ustawiaj `REBUILD_DB=true` na produkcji** — to wywołuje `reset_database`: DROP wszystkich tabel
//! i `seed_data` od zera (trwałe skasowanie danych). Backupy Turso: eksport z panelu / `turso db shell .dump`.
//!
//! Przy zwykłym starcie (bez `REBUILD_DB`) po migracjach wywoływane jest jednorazowe
//! `sync_all_athletes_bests_from_results`: pola `athletes.best_*` / `total_kg` ustawiane z najlepszego
//! **zatwierdzonego** wiersza w `results` (jak przy akceptacji wyniku). Ścieżka rebuild nie uruchamia
//! tej synchronizacji — seed może zostawiać ręczne rekordy do czasu dodania wyników.
//!
use libsql::Connection;
use tokio::time::{Duration, sleep};

/// Wykrywa stary moduł CMS z gałęzi `dev-cms` (page_key, cms_fields, cms_sections…).
async fn cms_legacy_schema_detected(conn: &Connection) -> Result<bool, String> {
    let mut legacy_tables = conn
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('cms_fields', 'cms_sections', 'cms_navigation')",
            (),
        )
        .await
        .map_err(|e| format!("cms legacy table check: {e}"))?;
    let legacy_count: i64 = legacy_tables
        .next()
        .await
        .map_err(|e| format!("cms legacy table row: {e}"))?
        .map(|r| r.get(0).unwrap_or(0))
        .unwrap_or(0);
    drop(legacy_tables);
    if legacy_count > 0 {
        return Ok(true);
    }

    let mut exists = conn
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'cms_pages'",
            (),
        )
        .await
        .map_err(|e| format!("cms_pages table check: {e}"))?;
    let table_exists: i64 = exists
        .next()
        .await
        .map_err(|e| format!("cms_pages table check row: {e}"))?
        .map(|r| r.get(0).unwrap_or(0))
        .unwrap_or(0);
    drop(exists);
    if table_exists == 0 {
        return Ok(false);
    }

    let mut cols = conn
        .query("PRAGMA table_info(cms_pages)", ())
        .await
        .map_err(|e| format!("cms_pages pragma: {e}"))?;
    let mut has_page_name = false;
    while let Some(row) = cols
        .next()
        .await
        .map_err(|e| format!("cms_pages pragma row: {e}"))?
    {
        let col: String = row.get(1).unwrap_or_default();
        if col == "page_name" {
            has_page_name = true;
        }
    }
    drop(cols);

    Ok(!has_page_name)
}

/// Stara gałąź `dev-cms` — tabele z FK i innymi kolumnami. DROP w poprawnej kolejności + FK off.
async fn migrate_cms_schema(conn: &Connection) -> Result<(), String> {
    if !cms_legacy_schema_detected(conn).await? {
        return Ok(());
    }

    slavia_warn!(
        "db.rs",
        "legacy CMS schema detected",
        "close other backend instances holding slavia.db lock",
        slavia_extra = "dropping cms_fields/cms_sections/cms_navigation"
    );
    let _ = conn.execute("PRAGMA busy_timeout = 3000", ()).await;
    exec0_retry_with_limit(conn, "PRAGMA foreign_keys = OFF", "cms foreign_keys off", 12).await?;

    for sql in [
        "DROP TABLE IF EXISTS cms_fields",
        "DROP TABLE IF EXISTS cms_sections",
        "DROP TABLE IF EXISTS cms_version_history",
        "DROP TABLE IF EXISTS cms_navigation_items",
        "DROP TABLE IF EXISTS cms_navigation",
        "DROP TABLE IF EXISTS cms_pages",
        "DROP TABLE IF EXISTS cms_variables",
    ] {
        exec0_retry_with_limit(conn, sql, "cms drop legacy tables", 12).await?;
    }

    let _ = conn.execute("PRAGMA foreign_keys = ON", ()).await;
    Ok(())
}

async fn cms_table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut cols = conn
        .query(&format!("PRAGMA table_info({table})"), ())
        .await
        .map_err(|e| format!("pragma table_info({table}): {e}"))?;
    while let Some(row) = cols
        .next()
        .await
        .map_err(|e| format!("pragma table_info({table}) row: {e}"))?
    {
        let col: String = row.get(1).unwrap_or_default();
        if col == column {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn create_cms_tables(conn: &Connection) -> Result<(), String> {
    let statements = [
        "CREATE TABLE IF NOT EXISTS cms_variables (
            id TEXT PRIMARY KEY,
            key TEXT NOT NULL UNIQUE,
            value TEXT NOT NULL,
            value_type TEXT NOT NULL DEFAULT 'text',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_cms_variables_key ON cms_variables(key)",
        "CREATE TABLE IF NOT EXISTS cms_pages (
            id TEXT PRIMARY KEY,
            page_name TEXT NOT NULL UNIQUE,
            fields TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS cms_navigation_items (
            id TEXT PRIMARY KEY,
            role TEXT NOT NULL,
            label TEXT NOT NULL,
            icon TEXT NOT NULL,
            url TEXT NOT NULL,
            order_index INTEGER NOT NULL DEFAULT 0,
            group_name TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_cms_nav_role_order ON cms_navigation_items(role, order_index)",
        "CREATE TABLE IF NOT EXISTS cms_version_history (
            id TEXT PRIMARY KEY,
            entity_type TEXT NOT NULL,
            entity_key TEXT NOT NULL,
            old_value TEXT,
            new_value TEXT,
            changed_by TEXT REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_cms_version_entity ON cms_version_history(entity_type, entity_key, created_at DESC)",
    ];
    for sql in statements {
        exec0_retry(conn, sql, "create_cms_tables").await?;
    }

    // Stary cms_pages (page_key) mógł przetrwać obok nowych tabel — indeks na page_name by wtedy padał.
    if cms_table_has_column(conn, "cms_pages", "page_name")
        .await
        .unwrap_or(false)
    {
        exec0_retry(
            conn,
            "CREATE INDEX IF NOT EXISTS idx_cms_pages_name ON cms_pages(page_name)",
            "create_cms_pages_index",
        )
        .await?;
    } else {
        let mut exists = conn
            .query(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'cms_pages'",
                (),
            )
            .await
            .map_err(|e| format!("cms_pages exists check: {e}"))?;
        let n: i64 = exists
            .next()
            .await
            .map_err(|e| format!("cms_pages exists row: {e}"))?
            .map(|r| r.get(0).unwrap_or(0))
            .unwrap_or(0);
        drop(exists);
        if n > 0 {
            slavia_warn!("db.rs", "cms_pages table missing page_name column", "table will be recreated automatically");
            exec0_retry(conn, "DROP TABLE IF EXISTS cms_pages", "cms_pages drop stale").await?;
            exec0_retry(
                conn,
                "CREATE TABLE cms_pages (
                    id TEXT PRIMARY KEY,
                    page_name TEXT NOT NULL UNIQUE,
                    fields TEXT NOT NULL DEFAULT '{}',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                )",
                "cms_pages recreate",
            )
            .await?;
            exec0_retry(
                conn,
                "CREATE INDEX IF NOT EXISTS idx_cms_pages_name ON cms_pages(page_name)",
                "create_cms_pages_index",
            )
            .await?;
        }
    }
    Ok(())
}

/// Usuwa duplikaty (athlete_id, session_date) przed utworzeniem indeksu UNIQUE — inaczej start pada na Turso.
async fn migrate_attendance_unique_index(conn: &Connection) -> Result<(), String> {
    let mut exists = conn
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'attendance_records'",
            (),
        )
        .await
        .map_err(|e| format!("attendance table check: {e}"))?;
    let n: i64 = if let Some(row) = exists
        .next()
        .await
        .map_err(|e| format!("attendance table row: {e}"))?
    {
        row.get(0).map_err(|e| format!("attendance table get: {e}"))?
    } else {
        0
    };
    drop(exists);
    if n == 0 {
        return Ok(());
    }

    let deduped = conn
        .execute(
            "DELETE FROM attendance_records WHERE id IN (
                SELECT id FROM (
                    SELECT id,
                        ROW_NUMBER() OVER (
                            PARTITION BY athlete_id, session_date
                            ORDER BY
                                CASE WHEN verification_state = 'verified' THEN 0 ELSE 1 END,
                                updated_at DESC,
                                rowid ASC
                        ) AS rn
                    FROM attendance_records
                ) WHERE rn > 1
            )",
            (),
        )
        .await
        .map_err(|e| format!("attendance dedupe: {e}"))?;
    if deduped > 0 {
        slavia_info!(
            "db.rs",
            "removed duplicate attendance_records before UNIQUE index",
            "no action needed",
            deduped
        );
    }

    exec0_retry(
        conn,
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_attendance_unique_athlete_session ON attendance_records(athlete_id, session_date)",
        "CREATE UNIQUE INDEX attendance_records",
    )
    .await
}

async fn exec0_retry_with_limit(
    conn: &Connection,
    sql: &str,
    label: &str,
    max_attempts: u32,
) -> Result<(), String> {
    // SQLite bywa chwilowo zablokowane (np. równoległe połączenie / statement).
    // Robimy krótki retry zamiast paniki na starcie.
    for attempt in 0..max_attempts {
        match conn.execute(sql, ()).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                let msg = e.to_string();
                let locked = msg.contains("database table is locked")
                    || msg.contains("database is locked")
                    || msg.contains("SQLITE_BUSY")
                    || msg.contains("SQLITE_LOCKED");
                if !locked || attempt + 1 == max_attempts {
                    return Err(format!("{label}: {msg}"));
                }
                // rosnący backoff z limitem ~1.5s
                let ms = (100 + (attempt as u64 * 80)).min(1500);
                sleep(Duration::from_millis(ms)).await;
            }
        }
    }
    Err(format!("{label}: exhausted retries"))
}

async fn exec0_retry(conn: &Connection, sql: &str, label: &str) -> Result<(), String> {
    exec0_retry_with_limit(conn, sql, label, 80).await
}

async fn migrate_remove_trainer_admin_role(conn: &Connection) -> Result<u64, String> {
    let mut rows = conn
        .query(
            "SELECT id, roles FROM users WHERE roles IS NOT NULL AND roles LIKE '%TrainerAdmin%'",
            (),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut updated = 0u64;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let id: String = row.get(0).map_err(|e| e.to_string())?;
        let roles_json: String = row.get(1).map_err(|e| e.to_string())?;
        let Ok(mut roles) = serde_json::from_str::<Vec<String>>(&roles_json) else {
            continue;
        };
        if !roles.iter().any(|r| r == "TrainerAdmin") {
            continue;
        }
        roles.retain(|r| r != "TrainerAdmin");
        if !roles.iter().any(|r| r == "Admin") {
            roles.push("Admin".into());
        }
        if !roles.iter().any(|r| r == "Trainer") {
            roles.push("Trainer".into());
        }
        let new_json = serde_json::to_string(&roles).map_err(|e| e.to_string())?;
        conn.execute("UPDATE users SET roles = ?1 WHERE id = ?2", (new_json, id))
            .await
            .map_err(|e| e.to_string())?;
        updated += 1;
    }
    Ok(updated)
}
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use chrono::{SecondsFormat, Utc};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct ExerciseDictionarySeedRow {
    id: String,
    name: String,
    category: String,
    description: String,
}

const EXERCISE_DICTIONARY_SEED_JSON: &str = include_str!("embed/exercise-dictionary-seed.json");

async fn insert_default_exercises(
    conn: &Connection,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<ExerciseDictionarySeedRow> = serde_json::from_str(EXERCISE_DICTIONARY_SEED_JSON)?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut inserted = 0u64;
    for row in rows {
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO exercises (id, name, category, description, video_url, created_at) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                (row.id, row.name, row.category, row.description, now.clone()),
            )
            .await?;
        inserted += n;
    }
    Ok(inserted)
}

/// Domyślny słownik ćwiczeń (plany treningowe) — `INSERT OR IGNORE` po stałych `id`.
async fn seed_default_exercises(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let n = insert_default_exercises(conn).await?;
    if n > 0 {
        slavia_info!("db.rs", "default exercise dictionary seeded", "no action needed", inserted = n);
    }
    Ok(())
}

const MIGRATION_SANITIZE_EXERCISES_UTF8_KEY: &str = "migration:sanitize_exercises_utf8_v1";

pub(crate) fn error_indicates_sqlite_corrupt(err: &dyn std::error::Error) -> bool {
    sqlite_corrupt_message(&err.to_string())
}

pub(crate) fn sqlite_corrupt_message(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("malformed")
        || m.contains("disk image")
        || m.contains("not a database")
}

async fn migration_flag_is_set(conn: &Connection, key: &str) -> bool {
    let Ok(mut rows) = conn
        .query(
            "SELECT value FROM system_settings WHERE key = ?1 LIMIT 1",
            [key.to_string()],
        )
        .await
    else {
        return false;
    };
    let Ok(Some(row)) = rows.next().await else {
        return false;
    };
    crate::sql_row::flex_string(&row, 0)
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
}

async fn set_migration_flag(conn: &Connection, key: &str) -> Result<(), libsql::Error> {
    conn.execute(
        "INSERT INTO system_settings (key, value) VALUES (?1, '1')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key.to_string()],
    )
    .await?;
    Ok(())
}

/// Przepisuje pola tekstowe `exercises` na poprawne UTF-8 (libsql panikuje na uszkodzonym TEXT).
async fn migrate_sanitize_exercises_utf8(conn: &Connection) -> Result<u64, String> {
    let mut rows = conn
        .query(
            "SELECT id,
                    CAST(name AS BLOB),
                    CAST(category AS BLOB),
                    CAST(description AS BLOB),
                    CAST(video_url AS BLOB),
                    CAST(created_at AS BLOB)
             FROM exercises
             WHERE id NOT LIKE 'dict-%'",
            (),
        )
        .await
        .map_err(|e| format!("migrate_sanitize_exercises_utf8 select: {e}"))?;

    let mut updated = 0u64;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("migrate_sanitize_exercises_utf8 row: {e}"))?
    {
        let id = crate::sql_row::required_lossy_string(&row, 0)
            .map_err(|e| format!("migrate_sanitize_exercises_utf8 id: {e}"))?;
        let name = crate::sql_row::required_lossy_string(&row, 1)
            .map_err(|e| format!("migrate_sanitize_exercises_utf8 name: {e}"))?;
        let category = crate::sql_row::lossy_opt_string(&row, 2)
            .map_err(|e| format!("migrate_sanitize_exercises_utf8 category: {e}"))?;
        let description = crate::sql_row::lossy_opt_string(&row, 3)
            .map_err(|e| format!("migrate_sanitize_exercises_utf8 description: {e}"))?;
        let video_url = crate::sql_row::lossy_opt_string(&row, 4)
            .map_err(|e| format!("migrate_sanitize_exercises_utf8 video_url: {e}"))?;
        let created_at = crate::sql_row::required_lossy_string(&row, 5)
            .map_err(|e| format!("migrate_sanitize_exercises_utf8 created_at: {e}"))?;

        let n = conn
            .execute(
                "UPDATE exercises SET name = ?1, category = ?2, description = ?3, video_url = ?4, created_at = ?5 WHERE id = ?6",
                (name, category, description, video_url, created_at, id),
            )
            .await
            .map_err(|e| format!("migrate_sanitize_exercises_utf8 update: {e}"))?;
        updated += n;
    }

    if updated > 0 {
        slavia_info!("db.rs", "exercises UTF-8 sanitized", "no action needed", updated);
    }
    Ok(updated)
}

/// Usuwa wpisy słownika `dict-*` i wstawia je ponownie z embed JSON (bez skanu całej tabeli).
async fn try_reseed_exercise_dictionary(conn: &Connection) -> Result<u64, String> {
    let deleted = conn
        .execute("DELETE FROM exercises WHERE id LIKE 'dict-%'", ())
        .await
        .map_err(|e| format!("reseed delete dict: {e}"))?;
    let inserted = insert_default_exercises(conn)
        .await
        .map_err(|e| format!("reseed insert dict: {e}"))?;
    Ok(deleted + inserted)
}

/// Jednorazowa migracja UTF-8 — nie blokuje startu przy uszkodzonej replice (Render/Turso).
async fn try_migrate_sanitize_exercises_utf8(conn: &Connection) {
    if migration_flag_is_set(conn, MIGRATION_SANITIZE_EXERCISES_UTF8_KEY).await {
        return;
    }

    match migrate_sanitize_exercises_utf8(conn).await {
        Ok(_) => {
            if let Err(e) = set_migration_flag(conn, MIGRATION_SANITIZE_EXERCISES_UTF8_KEY).await {
                slavia_warn!("db.rs", "failed to persist migration flag", "migration may rerun on next startup", error = %e);
            }
        }
        Err(e) => {
            slavia_warn!(
                "db.rs",
                "exercises UTF-8 migration skipped",
                "run migrate_refresh_corrupt_exercise_dictionary_seed or REBUILD_DB locally",
                error = %e
            );
            if sqlite_corrupt_message(&e) {
                match try_reseed_exercise_dictionary(conn).await {
                    Ok(n) if n > 0 => slavia_info!(
                        "db.rs",
                        "exercise dictionary reseeded after corrupt replica",
                        "verify exercises list in admin panel",
                        affected = n
                    ),
                    Ok(_) => {}
                    Err(re) => slavia_warn!(
                        "db.rs",
                        "exercise dictionary reseed failed",
                        "set REBUILD_DB=true locally or fix exercises table manually",
                        error = %re
                    ),
                }
            }
        }
    }
}

/// Przywraca polskie znaki w wpisach słownika (`dict-*`) z embed JSON, gdy w bazie są `U+FFFD`.
async fn migrate_refresh_corrupt_exercise_dictionary_seed(
    conn: &Connection,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<ExerciseDictionarySeedRow> = serde_json::from_str(EXERCISE_DICTIONARY_SEED_JSON)?;
    let mut updated = 0u64;
    for row in rows {
        let n = conn
            .execute(
                "UPDATE exercises SET name = ?1, category = ?2, description = ?3
                 WHERE id = ?4
                   AND (
                     instr(name, char(65533)) > 0
                     OR instr(COALESCE(category, ''), char(65533)) > 0
                     OR instr(COALESCE(description, ''), char(65533)) > 0
                   )",
                (row.name, row.category, row.description, row.id),
            )
            .await?;
        updated += n;
    }
    if updated > 0 {
        slavia_info!(
            "db.rs",
            "exercise dictionary polish characters restored from seed",
            "no action needed",
            updated
        );
    }
    Ok(updated)
}

/// Najlepszy zatwierdzony start (max `total`, przy remisie nowsza `date`) → rekord zawodnika.
pub async fn sync_athlete_bests_from_approved_conn(
    conn: &Connection,
    athlete_id: &str,
) -> Result<(), libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT snatch, clean_and_jerk, total FROM results \
             WHERE athlete_id = ?1 AND status = 'Approved' \
               AND (kind IS NULL OR kind = 'competition') \
             ORDER BY total DESC, date DESC LIMIT 1",
            [athlete_id.to_string()],
        )
        .await?;

    let row = rows.next().await?;

    match row {
        Some(r) => {
            let snatch: f64 = r.get(0)?;
            let clean_and_jerk: f64 = r.get(1)?;
            let total: f64 = r.get(2)?;
            conn.execute(
                "UPDATE athletes SET best_snatch_kg = ?1, best_clean_jerk_kg = ?2, total_kg = ?3 WHERE id = ?4",
                (snatch, clean_and_jerk, total, athlete_id.to_string()),
            )
            .await?;
        }
        None => {
            conn.execute(
                "UPDATE athletes SET best_snatch_kg = NULL, best_clean_jerk_kg = NULL, total_kg = NULL WHERE id = ?1",
                [athlete_id.to_string()],
            )
            .await?;
        }
    }
    Ok(())
}

/// Synchronizuje rekordy życiowe wielu zawodników jednym zapytaniem (batch approve).
pub async fn sync_athletes_bests_from_approved_batch_conn(
    conn: &Connection,
    athlete_ids: &[String],
) -> Result<(), libsql::Error> {
    if athlete_ids.is_empty() {
        return Ok(());
    }
    let placeholders = crate::sql_util::in_placeholders(athlete_ids.len());
    let sql = format!(
        "WITH ranked AS (
            SELECT athlete_id, snatch, clean_and_jerk, total,
                   ROW_NUMBER() OVER (
                     PARTITION BY athlete_id
                     ORDER BY total DESC, date DESC
                   ) AS rn
            FROM results
            WHERE status = 'Approved'
              AND (kind IS NULL OR kind = 'competition')
              AND athlete_id IN ({placeholders})
         )
         UPDATE athletes SET
           best_snatch_kg = (
             SELECT snatch FROM ranked r
             WHERE r.athlete_id = athletes.id AND r.rn = 1
           ),
           best_clean_jerk_kg = (
             SELECT clean_and_jerk FROM ranked r
             WHERE r.athlete_id = athletes.id AND r.rn = 1
           ),
           total_kg = (
             SELECT total FROM ranked r
             WHERE r.athlete_id = athletes.id AND r.rn = 1
           )
         WHERE athletes.id IN ({placeholders})"
    );
    conn.execute(&sql, athlete_ids.to_vec()).await?;
    Ok(())
}

/// Migracja przy starcie: wszystkie wiersze `athletes` — `best_*` wyłącznie z tabeli `results` (Approved).
pub async fn sync_all_athletes_bests_from_results(conn: &Connection) -> Result<u64, libsql::Error> {
    conn.execute(
        "WITH ranked AS (
            SELECT athlete_id, snatch, clean_and_jerk, total,
                   ROW_NUMBER() OVER (
                     PARTITION BY athlete_id
                     ORDER BY total DESC, date DESC
                   ) AS rn
            FROM results
            WHERE status = 'Approved'
              AND (kind IS NULL OR kind = 'competition')
         )
         UPDATE athletes SET
           best_snatch_kg = (
             SELECT snatch FROM ranked r
             WHERE r.athlete_id = athletes.id AND r.rn = 1
           ),
           best_clean_jerk_kg = (
             SELECT clean_and_jerk FROM ranked r
             WHERE r.athlete_id = athletes.id AND r.rn = 1
           ),
           total_kg = (
             SELECT total FROM ranked r
             WHERE r.athlete_id = athletes.id AND r.rn = 1
           )",
        (),
    )
    .await
}

pub async fn init_db(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Ustaw w `.env`: REBUILD_DB=true (jednorazowo, tylko dev), potem false — patrz `.env.example`
    let rebuild = std::env::var("REBUILD_DB").unwrap_or_default() == "true";

    if rebuild {
        if crate::production_guards::destructive_db_ops_blocked() {
            return Err(
                "REBUILD_DB=true is blocked when Turso/production database is configured".into(),
            );
        }
        slavia_warn!("db.rs", "REBUILD_DB requested", "never set REBUILD_DB=true on Turso/production");
        reset_database(conn).await?;
        return Ok(());
    }

    // Tworzenie tabel if not exists
    let create_tables = [
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            roles TEXT NOT NULL,
            avatar_url TEXT,
            ui_theme_preset TEXT,
            ui_color_mode TEXT,
            is_banned INTEGER NOT NULL DEFAULT 0,
            banned_at TEXT,
            banned_by_user_id TEXT,
            banned_reason TEXT,
            token_version INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS athletes (
            id TEXT PRIMARY KEY,
            user_id TEXT REFERENCES users(id),
            full_name TEXT NOT NULL,
            birth_year INTEGER,
            gender TEXT,
            weight_category TEXT,
            bodyweight REAL,
            best_snatch_kg REAL,
            best_clean_jerk_kg REAL,
            total_kg REAL,
            image_url TEXT,
            notes TEXT,
            profile_tagline TEXT,
            public_bio TEXT,
            is_active BOOLEAN DEFAULT 1,
            has_standing_order INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS competitions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            date TEXT NOT NULL,
            location TEXT NOT NULL,
            description TEXT,
            category TEXT DEFAULT 'club_event',
            category_override TEXT,
            status TEXT DEFAULT 'scheduled',
            external_source TEXT,
            external_ref TEXT,
            external_url TEXT,
            club_participates INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS competition_participants (
            competition_id TEXT NOT NULL,
            athlete_id TEXT NOT NULL,
            assigned_at TEXT NOT NULL,
            assigned_by TEXT,
            PRIMARY KEY (competition_id, athlete_id),
            FOREIGN KEY (competition_id) REFERENCES competitions(id) ON DELETE CASCADE,
            FOREIGN KEY (athlete_id) REFERENCES athletes(id) ON DELETE CASCADE
        )",
        "CREATE TABLE IF NOT EXISTS results (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id),
            competition_id TEXT REFERENCES competitions(id),
            snatch REAL NOT NULL,
            clean_and_jerk REAL NOT NULL,
            total REAL NOT NULL,
            status TEXT NOT NULL,
            date TEXT NOT NULL,
            bodyweight_kg REAL,
            squat_kg REAL,
            bench_kg REAL,
            deadlift_kg REAL,
            kind TEXT NOT NULL DEFAULT 'competition',
            location TEXT
        )",
        "CREATE TABLE IF NOT EXISTS posts (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            author_id TEXT NOT NULL REFERENCES users(id),
            image_url TEXT,
            created_at TEXT NOT NULL,
            published INTEGER NOT NULL DEFAULT 1
        )",
        "CREATE TABLE IF NOT EXISTS training_log_entries (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            session_date TEXT NOT NULL,
            title TEXT,
            notes TEXT NOT NULL,
            author_user_id TEXT REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS notifications (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            payload TEXT,
            created_at TEXT NOT NULL,
            is_read INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE INDEX IF NOT EXISTS idx_notifications_user_created ON notifications(user_id, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS attendance_records (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            session_date TEXT NOT NULL,
            status TEXT NOT NULL,
            source_role TEXT NOT NULL,
            created_by TEXT REFERENCES users(id),
            verified_by TEXT REFERENCES users(id),
            verification_state TEXT NOT NULL DEFAULT 'verified',
            note TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_attendance_athlete_date ON attendance_records(athlete_id, session_date DESC)",
        "CREATE TABLE IF NOT EXISTS system_audit_logs (
            id TEXT PRIMARY KEY,
            actor_user_id TEXT,
            actor_role TEXT,
            category TEXT NOT NULL,
            action TEXT NOT NULL,
            target_type TEXT,
            target_id TEXT,
            details TEXT,
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_audit_logs_created ON system_audit_logs(created_at DESC)",
        "CREATE TABLE IF NOT EXISTS feature_flags (
            name TEXT NOT NULL,
            value INTEGER NOT NULL,
            user_id TEXT,
            updated_by TEXT REFERENCES users(id),
            updated_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (name, user_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_feature_flags_user ON feature_flags(user_id, name)",
        "CREATE TABLE IF NOT EXISTS rate_limit_hits (
            id TEXT PRIMARY KEY,
            scope_key TEXT NOT NULL,
            bucket TEXT NOT NULL,
            hit_at_ms INTEGER NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_rate_limit_hits_scope ON rate_limit_hits(scope_key, bucket, hit_at_ms)",
        "CREATE TABLE IF NOT EXISTS feature_flag_events (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            value INTEGER NOT NULL,
            user_id TEXT,
            actor_user_id TEXT REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_feature_flag_events_created ON feature_flag_events(created_at DESC)",
        "CREATE TABLE IF NOT EXISTS chat_threads (
            id TEXT PRIMARY KEY,
            athlete_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            trainer_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            title TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_threads_pair ON chat_threads(athlete_user_id, trainer_user_id)",
        "CREATE TABLE IF NOT EXISTS chat_messages (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL REFERENCES chat_threads(id) ON DELETE CASCADE,
            sender_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            body TEXT NOT NULL,
            created_at TEXT NOT NULL,
            deleted_by_sender INTEGER NOT NULL DEFAULT 0,
            deleted_by_receiver INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS chat_reads (
            thread_id TEXT NOT NULL REFERENCES chat_threads(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            last_read_at TEXT NOT NULL,
            PRIMARY KEY (thread_id, user_id)
        )",
        "CREATE TABLE IF NOT EXISTS recurring_training_cancellations (
            session_date TEXT PRIMARY KEY,
            cancelled_at TEXT NOT NULL,
            cancelled_by TEXT REFERENCES users(id),
            status TEXT NOT NULL DEFAULT 'cancelled'
        )",
        "CREATE TABLE IF NOT EXISTS announcements (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            pinned INTEGER NOT NULL DEFAULT 0,
            sort_order INTEGER NOT NULL DEFAULT 0,
            published INTEGER NOT NULL DEFAULT 1,
            author_id TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS membership_payments (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            month TEXT NOT NULL,
            amount_pln REAL,
            note TEXT,
            status TEXT NOT NULL,
            created_by_user_id TEXT REFERENCES users(id),
            created_at TEXT NOT NULL,
            approved_by_user_id TEXT REFERENCES users(id),
            approved_at TEXT
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_membership_payments_unique ON membership_payments(athlete_id, month)",
        "CREATE INDEX IF NOT EXISTS idx_membership_payments_athlete_month ON membership_payments(athlete_id, month)",
        "CREATE INDEX IF NOT EXISTS idx_membership_payments_status_created ON membership_payments(status, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_membership_payments_month_status ON membership_payments(month, status)",
        "CREATE TABLE IF NOT EXISTS gallery_photos (
            id TEXT PRIMARY KEY,
            image_url TEXT NOT NULL,
            media_type TEXT NOT NULL DEFAULT 'image',
            caption TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            published INTEGER NOT NULL DEFAULT 1,
            author_id TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS coach_comments (
            id TEXT PRIMARY KEY,
            target_type TEXT NOT NULL,
            target_id TEXT NOT NULL,
            body TEXT NOT NULL,
            author_user_id TEXT REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_coach_comments_target ON coach_comments(target_type, target_id, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS training_plans (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            title TEXT NOT NULL,
            goal TEXT,
            week_start TEXT NOT NULL,
            duration_weeks INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL DEFAULT 'planned',
            coach_note TEXT,
            athlete_note TEXT,
            progress_percent INTEGER NOT NULL DEFAULT 0,
            created_by TEXT REFERENCES users(id),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_training_plans_athlete_week ON training_plans(athlete_id, week_start DESC)",
        "CREATE TABLE IF NOT EXISTS exercises (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            category TEXT,
            description TEXT,
            video_url TEXT,
            created_at TEXT NOT NULL
        )",
        // Osobny system „Inne ćwiczenia” — zgłoszenia + historia (nie dotyka tabeli `results`)
        "CREATE TABLE IF NOT EXISTS exercise_submissions (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            exercise_id TEXT NOT NULL REFERENCES exercises(id) ON DELETE RESTRICT,
            value REAL NOT NULL,
            unit TEXT NOT NULL DEFAULT 'kg',
            performed_at TEXT NOT NULL,
            notes TEXT,
            status TEXT NOT NULL DEFAULT 'Pending',
            reviewed_by_user_id TEXT REFERENCES users(id),
            reviewed_at TEXT,
            review_note TEXT,
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_exercise_submissions_athlete_created ON exercise_submissions(athlete_id, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_exercise_submissions_status_created ON exercise_submissions(status, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_exercise_submissions_exercise_created ON exercise_submissions(exercise_id, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS exercise_results_history (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            exercise_id TEXT NOT NULL REFERENCES exercises(id) ON DELETE RESTRICT,
            value REAL NOT NULL,
            unit TEXT NOT NULL DEFAULT 'kg',
            performed_at TEXT NOT NULL,
            submission_id TEXT REFERENCES exercise_submissions(id) ON DELETE SET NULL,
            approved_by_user_id TEXT REFERENCES users(id),
            approved_at TEXT NOT NULL,
            notes TEXT,
            review_note TEXT,
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_exercise_history_exercise_value ON exercise_results_history(exercise_id, value DESC)",
        "CREATE INDEX IF NOT EXISTS idx_exercise_history_athlete_exercise ON exercise_results_history(athlete_id, exercise_id, value DESC)",
        "CREATE INDEX IF NOT EXISTS idx_exercise_history_created ON exercise_results_history(created_at DESC)",
        "CREATE TABLE IF NOT EXISTS training_plan_items (
            id TEXT PRIMARY KEY,
            plan_id TEXT NOT NULL REFERENCES training_plans(id) ON DELETE CASCADE,
            week_number INTEGER NOT NULL DEFAULT 1,
            day_of_week INTEGER NOT NULL,
            exercise_id TEXT REFERENCES exercises(id),
            custom_exercise_name TEXT,
            sets INTEGER,
            reps INTEGER,
            intensity_percent REAL,
            weight_kg REAL,
            notes TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS recovery_logs (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            date TEXT NOT NULL,
            sleep_hours REAL NOT NULL,
            fatigue_level INTEGER NOT NULL,
            soreness_level INTEGER NOT NULL,
            readiness_level INTEGER NOT NULL,
            note TEXT,
            created_at TEXT NOT NULL
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_recovery_athlete_date ON recovery_logs(athlete_id, date)",
        "CREATE TABLE IF NOT EXISTS contact_messages (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            email TEXT,
            phone TEXT,
            message TEXT NOT NULL,
            created_at TEXT NOT NULL,
            is_read INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS club_votes (
            id TEXT PRIMARY KEY,
            voter_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            month TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_club_votes_voter_month ON club_votes(voter_user_id, month)",
        "CREATE TABLE IF NOT EXISTS system_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    ];

    for sql in create_tables {
        conn.execute(sql, ()).await?;
    }

    migrate_cms_schema(conn)
        .await
        .map_err(|e| format!("migrate_cms_schema: {e}"))?;
    create_cms_tables(conn)
        .await
        .map_err(|e| format!("create_cms_tables: {e}"))?;

    migrate_attendance_unique_index(conn)
        .await
        .map_err(|e| format!("migrate_attendance_unique_index: {e}"))?;

    for sql in [
        "CREATE INDEX IF NOT EXISTS idx_results_status_kind ON results(status, kind)",
        "CREATE INDEX IF NOT EXISTS idx_results_athlete_status ON results(athlete_id, status)",
        "CREATE INDEX IF NOT EXISTS idx_results_status_kind_date ON results(status, kind, date DESC, total DESC)",
        "CREATE INDEX IF NOT EXISTS idx_results_date ON results(date DESC)",
        "CREATE INDEX IF NOT EXISTS idx_athletes_active ON athletes(is_active)",
        "CREATE INDEX IF NOT EXISTS idx_athletes_gender ON athletes(gender)",
        "CREATE INDEX IF NOT EXISTS idx_athletes_weight_category ON athletes(weight_category)",
        "CREATE INDEX IF NOT EXISTS idx_athletes_active_total ON athletes(is_active, total_kg)",
    ] {
        let _ = conn.execute(sql, ()).await;
    }

    let _ = conn
        .execute(
            "ALTER TABLE recurring_training_cancellations ADD COLUMN status TEXT DEFAULT 'cancelled'",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "UPDATE recurring_training_cancellations SET status = 'cancelled' WHERE status IS NULL OR trim(status) = ''",
            (),
        )
        .await;

    // Migrate: add category and status columns if missing (safe for existing DBs)
    let _ = conn
        .execute(
            "ALTER TABLE competitions ADD COLUMN category TEXT DEFAULT 'club_event'",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE competitions ADD COLUMN status TEXT DEFAULT 'scheduled'",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE competitions ADD COLUMN category_override TEXT",
            (),
        )
        .await;

    // Migrate: kolumna is_active przy starszych instancjach Turso (bez niej SELECT na liście publicznej się wywali)
    let _ = conn
        .execute(
            "ALTER TABLE athletes ADD COLUMN is_active BOOLEAN DEFAULT 1",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "UPDATE athletes SET is_active = 1 WHERE is_active IS NULL",
            (),
        )
        .await;

    // Migrate: gender — starsze tabele athletes bez tej kolumny (CREATE TABLE IF NOT EXISTS jej nie doda)
    let _ = conn
        .execute("ALTER TABLE athletes ADD COLUMN gender TEXT", ())
        .await;

    let _ = conn
        .execute("ALTER TABLE athletes ADD COLUMN profile_tagline TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE athletes ADD COLUMN public_bio TEXT", ())
        .await;
    // „Przelew stały" — flaga: czy zawodnik ma zlecenie stałe na składkę i automatycznie przy
    // pierwszej okazji każdego miesiąca system zapisze mu Approved-payment.
    let _ = conn
        .execute(
            "ALTER TABLE athletes ADD COLUMN has_standing_order INTEGER NOT NULL DEFAULT 0",
            (),
        )
        .await;

    let _ = conn
        .execute("ALTER TABLE users ADD COLUMN avatar_url TEXT", ())
        .await;

    let _ = conn
        .execute("ALTER TABLE users ADD COLUMN ui_theme_preset TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE users ADD COLUMN ui_color_mode TEXT", ())
        .await;

    // Banowanie kont (egzekwowane w middleware auth)
    let _ = conn
        .execute(
            "ALTER TABLE users ADD COLUMN is_banned INTEGER NOT NULL DEFAULT 0",
            (),
        )
        .await;
    let _ = conn
        .execute("ALTER TABLE users ADD COLUMN banned_at TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE users ADD COLUMN banned_by_user_id TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE users ADD COLUMN banned_reason TEXT", ())
        .await;

    let _ = conn
        .execute("ALTER TABLE users ADD COLUMN totp_secret TEXT", ())
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0",
            (),
        )
        .await;

    // Token version for "logout from all devices"
    let _ = conn
        .execute(
            "ALTER TABLE users ADD COLUMN token_version INTEGER NOT NULL DEFAULT 0",
            (),
        )
        .await;

    let _ = conn
        .execute(
            "ALTER TABLE posts ADD COLUMN published INTEGER NOT NULL DEFAULT 1",
            (),
        )
        .await;

    let _ = conn
        .execute(
            "CREATE INDEX IF NOT EXISTS idx_posts_published_created ON posts(published, created_at DESC)",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "CREATE INDEX IF NOT EXISTS idx_announcements_published_list ON announcements(published, pinned DESC, sort_order ASC, created_at DESC)",
            (),
        )
        .await;

    let _ = conn
        .execute(
            "ALTER TABLE competitions ADD COLUMN external_source TEXT",
            (),
        )
        .await;
    let _ = conn
        .execute("ALTER TABLE competitions ADD COLUMN external_ref TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE competitions ADD COLUMN external_url TEXT", ())
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE competitions ADD COLUMN club_participates INTEGER NOT NULL DEFAULT 0",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_competitions_external_ref ON competitions(external_source, external_ref) WHERE external_source IS NOT NULL AND external_ref IS NOT NULL",
            (),
        )
        .await;

    let _ = conn
        .execute(
            "ALTER TABLE gallery_photos ADD COLUMN media_type TEXT NOT NULL DEFAULT 'image'",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "UPDATE gallery_photos SET media_type = 'image' WHERE media_type IS NULL",
            (),
        )
        .await;

    let _ = conn
        .execute("ALTER TABLE results ADD COLUMN squat_kg REAL", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE results ADD COLUMN bench_kg REAL", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE results ADD COLUMN deadlift_kg REAL", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE results ADD COLUMN bodyweight_kg REAL", ())
        .await;
    // Rozróżnienie wpisów: 'competition' (publiczne, ranking, public-board) vs 'training' (tylko po zalogowaniu).
    let _ = conn
        .execute(
            "ALTER TABLE results ADD COLUMN kind TEXT NOT NULL DEFAULT 'competition'",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "UPDATE results SET kind = 'competition' WHERE kind IS NULL OR kind = ''",
            (),
        )
        .await;
    // Miejsce zawodów / treningu. Dla `kind='training'` zawsze 'Slavia' (sala klubowa).
    let _ = conn
        .execute("ALTER TABLE results ADD COLUMN location TEXT", ())
        .await;
    // Backfill: starsze wpisy treningowe mogły mieć `location IS NULL` lub puste — uzupełnij.
    let _ = conn
        .execute(
            "UPDATE results SET location = 'Slavia' WHERE kind = 'training' AND (location IS NULL OR trim(location) = '')",
            (),
        )
        .await;
    let _ = conn
        .execute("ALTER TABLE chat_threads ADD COLUMN title TEXT", ())
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE notifications ADD COLUMN is_read INTEGER NOT NULL DEFAULT 0",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "UPDATE attendance_records SET status = 'nieobecny' WHERE status IN ('spóźniony', 'spozniony', 'late')",
            (),
        )
        .await;
    let _ = conn
        .execute("ALTER TABLE users ADD COLUMN last_seen_at TEXT", ())
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE training_plans ADD COLUMN duration_weeks INTEGER NOT NULL DEFAULT 1",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE training_plan_items ADD COLUMN week_number INTEGER NOT NULL DEFAULT 1",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "CREATE TABLE IF NOT EXISTS chat_message_reactions (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                emoji TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_reaction_unique ON chat_message_reactions(message_id, user_id, emoji)",
            (),
        )
        .await;

    let rebuild_db = std::env::var("REBUILD_DB")
        .ok()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    if rebuild_db {
        let n = sync_all_athletes_bests_from_results(conn)
            .await
            .map_err(|e| format!("sync_all_athletes_bests_from_results: {e}"))?;
        slavia_info!(
            "db.rs",
            "athlete bests synced from results after REBUILD_DB",
            "no action needed",
            sqlite_changes = n
        );
    }

    // Migracja: role → roles (JSON array) — wyłącznie dla starych baz z kolumną `role`
    // (świeże instalacje mają od razu `roles`; SELECT role wtedy kończy się błędem „no such column”).
    let _ = conn.execute("PRAGMA busy_timeout = 5000", ()).await;
    let mut pragma = conn
        .query(
            "SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'role'",
            (),
        )
        .await
        .map_err(|e| format!("pragma_table_info(users): {e}"))?;
    let role_column_exists: i64 = pragma
        .next()
        .await
        .map_err(|e| format!("pragma_table_info row: {e}"))?
        .map(|row| row.get::<i64>(0))
        .transpose()
        .map_err(|e| format!("pragma_table_info get: {e}"))?
        .unwrap_or(0);
    // ważne: zwolnij statement zanim wejdziemy w DDL
    drop(pragma);

    let mut migrated = 0;
    if role_column_exists > 0 {
        let mut pragma_roles = conn
            .query(
                "SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'roles'",
                (),
            )
            .await
            .map_err(|e| format!("pragma_table_info(users) roles: {e}"))?;
        let roles_column_exists: i64 = pragma_roles
            .next()
            .await
            .map_err(|e| format!("pragma_table_info roles row: {e}"))?
            .map(|row| row.get::<i64>(0))
            .transpose()
            .map_err(|e| format!("pragma_table_info roles get: {e}"))?
            .unwrap_or(0);

        // Stary schemat: jedna kolumna `role` — dopiero potem dodajemy `roles` i kopiujemy wartości.
        if roles_column_exists == 0 {
            conn.execute("ALTER TABLE users ADD COLUMN roles TEXT", ())
                .await
                .map_err(|e| format!("ALTER ADD users.roles: {e}"))?;
        }

        let mut rows = conn
            .query("SELECT id, role FROM users WHERE role IS NOT NULL", ())
            .await
            .map_err(|e| format!("select users for role migration: {e}"))?;
        while let Some(row) = rows.next().await.map_err(|e| format!("next row: {e}"))? {
            let id: String = row.get(0).map_err(|e| format!("get id: {e}"))?;
            let role: String = row.get(1).map_err(|e| format!("get role: {e}"))?;
            let roles_json = format!("[\"{}\"]", role);
            conn.execute(
                "UPDATE users SET roles = ?1 WHERE id = ?2",
                (roles_json, id),
            )
            .await
            .map_err(|e| format!("update roles: {e}"))?;
            migrated += 1;
        }
    }
    slavia_info!("db.rs", "migrated users.role column to roles JSON", "no action needed", migrated);

    // Usunięcie legacy kolumny `users.role` (SQLite nie wspiera DROP COLUMN -> rekonstrukcja tabeli)
    if role_column_exists > 0 {
        slavia_info!("db.rs", "rebuilding users table without legacy role column", "wait for migration to finish");
        // Wyłącz FK na czas rekonstrukcji.
        let _ = conn.execute("PRAGMA foreign_keys = OFF", ()).await;
        // BEGIN IMMEDIATE — weź write-lock od razu (mniej szans na DROP/ALTER lock).
        exec0_retry(
            conn,
            "BEGIN IMMEDIATE",
            "BEGIN IMMEDIATE drop users.role migration",
        )
        .await?;

        // Nowy schemat bez `role`
        exec0_retry(
            conn,
            "CREATE TABLE users__new (
                id TEXT PRIMARY KEY,
                username TEXT UNIQUE NOT NULL,
                password_hash TEXT NOT NULL,
                roles TEXT NOT NULL,
                avatar_url TEXT,
                ui_theme_preset TEXT,
                ui_color_mode TEXT,
                is_banned INTEGER NOT NULL DEFAULT 0,
                banned_at TEXT,
                banned_by_user_id TEXT,
                banned_reason TEXT,
                token_version INTEGER NOT NULL DEFAULT 0
            )",
            "CREATE TABLE users__new",
        )
        .await?;

        // Kopiowanie danych: roles -> roles; jeśli roles brak, to role -> JSON; ostatecznie fallback.
        exec0_retry(
            conn,
            "INSERT INTO users__new (
                id, username, password_hash, roles, avatar_url, ui_theme_preset, ui_color_mode,
                is_banned, banned_at, banned_by_user_id, banned_reason, token_version
             )
             SELECT
                id,
                username,
                password_hash,
                COALESCE(
                    roles,
                    CASE
                        WHEN role IS NOT NULL AND TRIM(role) <> '' THEN ('[\"' || role || '\"]')
                        ELSE '[\"Athlete\"]'
                    END
                ) AS roles,
                avatar_url,
                ui_theme_preset,
                ui_color_mode,
                COALESCE(is_banned, 0),
                banned_at,
                banned_by_user_id,
                banned_reason,
                COALESCE(token_version, 0)
             FROM users",
            "INSERT users__new",
        )
        .await?;

        exec0_retry(conn, "DROP TABLE users", "DROP TABLE users").await?;
        exec0_retry(
            conn,
            "ALTER TABLE users__new RENAME TO users",
            "RENAME users__new",
        )
        .await?;

        exec0_retry(conn, "COMMIT", "COMMIT drop users.role migration").await?;
        let _ = conn.execute("PRAGMA foreign_keys = ON", ()).await;
        slavia_info!("db.rs", "legacy users.role column removed", "no action needed");
    }

    let trainer_admin_fix = migrate_remove_trainer_admin_role(conn)
        .await
        .map_err(|e| format!("migrate_remove_trainer_admin_role: {e}"))?;
    slavia_info!(
        "db.rs",
        "TrainerAdmin role migrated to Admin+Trainer",
        "no action needed",
        updated = trainer_admin_fix
    );

    let _ = conn
        .execute(
            "UPDATE feature_flags SET name = 'olympic_coach' WHERE name = 'gemini_olympic_coach'",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "UPDATE feature_flag_events SET name = 'olympic_coach' WHERE name = 'gemini_olympic_coach'",
            (),
        )
        .await;

    // Migracja: usunięcie users.email (nie używamy maili) — zachowaj wszystkie inne dane kont.
    {
        let mut pragma = conn
            .query(
                "SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'email'",
                (),
            )
            .await
            .map_err(|e| format!("pragma_table_info(users) email: {e}"))?;
        let email_column_exists: i64 = pragma
            .next()
            .await
            .map_err(|e| format!("pragma users email row: {e}"))?
            .map(|row| row.get::<i64>(0))
            .transpose()
            .map_err(|e| format!("pragma users email get: {e}"))?
            .unwrap_or(0);
        drop(pragma);

        if email_column_exists > 0 {
            slavia_info!("db.rs", "rebuilding users table without email column", "wait for migration to finish");
            let _ = conn.execute("PRAGMA foreign_keys = OFF", ()).await;
            exec0_retry(
                conn,
                "BEGIN IMMEDIATE",
                "BEGIN IMMEDIATE drop users.email migration",
            )
            .await?;

            exec0_retry(
                conn,
                "CREATE TABLE users__new (
                    id TEXT PRIMARY KEY,
                    username TEXT UNIQUE NOT NULL,
                    password_hash TEXT NOT NULL,
                    roles TEXT NOT NULL,
                    avatar_url TEXT,
                    ui_theme_preset TEXT,
                    ui_color_mode TEXT,
                    is_banned INTEGER NOT NULL DEFAULT 0,
                    banned_at TEXT,
                    banned_by_user_id TEXT,
                    banned_reason TEXT
                )",
                "CREATE TABLE users__new (no email)",
            )
            .await?;

            exec0_retry(
                conn,
                "INSERT INTO users__new (
                    id, username, password_hash, roles, avatar_url, ui_theme_preset, ui_color_mode,
                    is_banned, banned_at, banned_by_user_id, banned_reason
                 )
                 SELECT
                    id, username, password_hash, roles, avatar_url, ui_theme_preset, ui_color_mode,
                    COALESCE(is_banned, 0), banned_at, banned_by_user_id, banned_reason
                 FROM users",
                "INSERT users__new (no email)",
            )
            .await?;

            exec0_retry(conn, "DROP TABLE users", "DROP TABLE users (no email)").await?;
            exec0_retry(
                conn,
                "ALTER TABLE users__new RENAME TO users",
                "RENAME users__new",
            )
            .await?;
            exec0_retry(conn, "COMMIT", "COMMIT drop users.email migration").await?;
            let _ = conn.execute("PRAGMA foreign_keys = ON", ()).await;
            slavia_info!("db.rs", "users.email column removed", "no action needed");
        }
    }

    // Migracja: przywrócenie contact_messages.email (formularz publiczny — przydatne do odpowiedzi).
    {
        let mut pragma = conn
            .query(
                "SELECT COUNT(*) FROM pragma_table_info('contact_messages') WHERE name = 'email'",
                (),
            )
            .await
            .map_err(|e| format!("pragma_table_info(contact_messages) email: {e}"))?;
        let email_column_exists: i64 = pragma
            .next()
            .await
            .map_err(|e| format!("pragma contact_messages email row: {e}"))?
            .map(|row| row.get::<i64>(0))
            .transpose()
            .map_err(|e| format!("pragma contact_messages email get: {e}"))?
            .unwrap_or(0);
        drop(pragma);

        if email_column_exists == 0 {
            slavia_info!("db.rs", "adding contact_messages.email column", "wait for migration to finish");
            let _ = conn
                .execute("ALTER TABLE contact_messages ADD COLUMN email TEXT", ())
                .await;
        }
    }

    seed_default_exercises(conn).await?;

    if let Err(e) = crate::db_migrations::apply_pending(conn).await {
        return Err(format!("SQL migrations: {e}").into());
    }

    try_migrate_sanitize_exercises_utf8(conn).await;

    if let Err(e) = migrate_refresh_corrupt_exercise_dictionary_seed(conn).await {
        slavia_warn!(
            "db.rs",
            "exercise dictionary refresh skipped",
            "best-effort migration; verify exercises manually if needed",
            error = %e
        );
    }

    if let Err(e) = exercises_table_probe(conn).await {
        if sqlite_corrupt_message(&e) {
            return Err(format!("replika SQLite uszkodzona (exercises): {e}").into());
        }
        slavia_warn!("db.rs", "exercises table probe failed after migrations", "continuing startup best-effort", error = %e);
    }

    Ok(())
}

/// Lekki odczyt kilku wierszy — wykrywa uszkodzoną replikę, której `integrity_check` nie złapał.
async fn exercises_table_probe(conn: &Connection) -> Result<(), String> {
    let mut rows = conn
        .query("SELECT id FROM exercises ORDER BY id LIMIT 5", ())
        .await
        .map_err(|e| format!("exercises probe: {e}"))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("exercises probe row: {e}"))?
    {
        crate::sql_row::flex_string(&row, 0)
            .map_err(|e| format!("exercises probe id: {e}"))?;
    }
    Ok(())
}

pub async fn reset_database(
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let drop_tables = [
        "DROP TABLE IF EXISTS exercise_results_history",
        "DROP TABLE IF EXISTS exercise_submissions",
        "DROP TABLE IF EXISTS membership_payments",
        "DROP TABLE IF EXISTS notifications",
        "DROP TABLE IF EXISTS chat_reads",
        "DROP TABLE IF EXISTS chat_messages",
        "DROP TABLE IF EXISTS chat_threads",
        "DROP TABLE IF EXISTS attendance_records",
        "DROP TABLE IF EXISTS system_audit_logs",
        "DROP TABLE IF EXISTS feature_flag_events",
        "DROP TABLE IF EXISTS feature_flags",
        "DROP TABLE IF EXISTS results",
        "DROP TABLE IF EXISTS competition_participants",
        "DROP TABLE IF EXISTS recurring_training_cancellations",
        "DROP TABLE IF EXISTS training_log_entries",
        "DROP TABLE IF EXISTS contact_messages",
        "DROP TABLE IF EXISTS gallery_photos",
        "DROP TABLE IF EXISTS coach_comments",
        "DROP TABLE IF EXISTS training_plan_items",
        "DROP TABLE IF EXISTS exercises",
        "DROP TABLE IF EXISTS training_plans",
        "DROP TABLE IF EXISTS recovery_logs",
        "DROP TABLE IF EXISTS announcements",
        "DROP TABLE IF EXISTS posts",
        "DROP TABLE IF EXISTS competitions",
        "DROP TABLE IF EXISTS athletes",
        "DROP TABLE IF EXISTS users",
    ];
    for sql in drop_tables {
        let _ = conn.execute(sql, ()).await;
    }

    let create_tables = [
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            roles TEXT NOT NULL,
            avatar_url TEXT,
            ui_theme_preset TEXT,
            ui_color_mode TEXT,
            is_banned INTEGER NOT NULL DEFAULT 0,
            banned_at TEXT,
            banned_by_user_id TEXT,
            banned_reason TEXT,
            totp_secret TEXT,
            totp_enabled INTEGER NOT NULL DEFAULT 0,
            token_version INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS athletes (
            id TEXT PRIMARY KEY,
            user_id TEXT REFERENCES users(id),
            full_name TEXT NOT NULL,
            birth_year INTEGER,
            gender TEXT,
            weight_category TEXT,
            bodyweight REAL,
            best_snatch_kg REAL,
            best_clean_jerk_kg REAL,
            total_kg REAL,
            image_url TEXT,
            notes TEXT,
            profile_tagline TEXT,
            public_bio TEXT,
            is_active BOOLEAN DEFAULT 1,
            has_standing_order INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS competitions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            date TEXT NOT NULL,
            location TEXT NOT NULL,
            description TEXT,
            category TEXT DEFAULT 'club_event',
            category_override TEXT,
            status TEXT DEFAULT 'scheduled',
            external_source TEXT,
            external_ref TEXT,
            external_url TEXT,
            club_participates INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS competition_participants (
            competition_id TEXT NOT NULL,
            athlete_id TEXT NOT NULL,
            assigned_at TEXT NOT NULL,
            assigned_by TEXT,
            PRIMARY KEY (competition_id, athlete_id),
            FOREIGN KEY (competition_id) REFERENCES competitions(id) ON DELETE CASCADE,
            FOREIGN KEY (athlete_id) REFERENCES athletes(id) ON DELETE CASCADE
        )",
        "CREATE TABLE IF NOT EXISTS results (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id),
            competition_id TEXT REFERENCES competitions(id),
            snatch REAL NOT NULL,
            clean_and_jerk REAL NOT NULL,
            total REAL NOT NULL,
            status TEXT NOT NULL,
            date TEXT NOT NULL,
            squat_kg REAL,
            bench_kg REAL,
            deadlift_kg REAL,
            kind TEXT NOT NULL DEFAULT 'competition',
            location TEXT
        )",
        "CREATE TABLE IF NOT EXISTS posts (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            author_id TEXT NOT NULL REFERENCES users(id),
            image_url TEXT,
            created_at TEXT NOT NULL,
            published INTEGER NOT NULL DEFAULT 1
        )",
        "CREATE TABLE IF NOT EXISTS training_log_entries (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            session_date TEXT NOT NULL,
            title TEXT,
            notes TEXT NOT NULL,
            author_user_id TEXT REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS notifications (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            payload TEXT,
            created_at TEXT NOT NULL,
            is_read INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE INDEX IF NOT EXISTS idx_notifications_user_created ON notifications(user_id, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS attendance_records (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            session_date TEXT NOT NULL,
            status TEXT NOT NULL,
            source_role TEXT NOT NULL,
            created_by TEXT REFERENCES users(id),
            verified_by TEXT REFERENCES users(id),
            verification_state TEXT NOT NULL DEFAULT 'verified',
            note TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_attendance_athlete_date ON attendance_records(athlete_id, session_date DESC)",
        "CREATE TABLE IF NOT EXISTS system_audit_logs (
            id TEXT PRIMARY KEY,
            actor_user_id TEXT,
            actor_role TEXT,
            category TEXT NOT NULL,
            action TEXT NOT NULL,
            target_type TEXT,
            target_id TEXT,
            details TEXT,
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_audit_logs_created ON system_audit_logs(created_at DESC)",
        "CREATE TABLE IF NOT EXISTS feature_flags (
            name TEXT NOT NULL,
            value INTEGER NOT NULL,
            user_id TEXT,
            updated_by TEXT REFERENCES users(id),
            updated_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (name, user_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_feature_flags_user ON feature_flags(user_id, name)",
        "CREATE TABLE IF NOT EXISTS rate_limit_hits (
            id TEXT PRIMARY KEY,
            scope_key TEXT NOT NULL,
            bucket TEXT NOT NULL,
            hit_at_ms INTEGER NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_rate_limit_hits_scope ON rate_limit_hits(scope_key, bucket, hit_at_ms)",
        "CREATE TABLE IF NOT EXISTS feature_flag_events (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            value INTEGER NOT NULL,
            user_id TEXT,
            actor_user_id TEXT REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_feature_flag_events_created ON feature_flag_events(created_at DESC)",
        "CREATE TABLE IF NOT EXISTS chat_threads (
            id TEXT PRIMARY KEY,
            athlete_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            trainer_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            title TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_threads_pair ON chat_threads(athlete_user_id, trainer_user_id)",
        "CREATE TABLE IF NOT EXISTS chat_messages (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL REFERENCES chat_threads(id) ON DELETE CASCADE,
            sender_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            body TEXT NOT NULL,
            created_at TEXT NOT NULL,
            deleted_by_sender INTEGER NOT NULL DEFAULT 0,
            deleted_by_receiver INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE IF NOT EXISTS chat_reads (
            thread_id TEXT NOT NULL REFERENCES chat_threads(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            last_read_at TEXT NOT NULL,
            PRIMARY KEY (thread_id, user_id)
        )",
        "CREATE TABLE IF NOT EXISTS recurring_training_cancellations (
            session_date TEXT PRIMARY KEY,
            cancelled_at TEXT NOT NULL,
            cancelled_by TEXT REFERENCES users(id),
            status TEXT NOT NULL DEFAULT 'cancelled'
        )",
        "CREATE TABLE IF NOT EXISTS announcements (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            pinned INTEGER NOT NULL DEFAULT 0,
            sort_order INTEGER NOT NULL DEFAULT 0,
            published INTEGER NOT NULL DEFAULT 1,
            author_id TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS membership_payments (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            month TEXT NOT NULL,
            amount_pln REAL,
            note TEXT,
            status TEXT NOT NULL,
            created_by_user_id TEXT REFERENCES users(id),
            created_at TEXT NOT NULL,
            approved_by_user_id TEXT REFERENCES users(id),
            approved_at TEXT
        )",
        "CREATE INDEX IF NOT EXISTS idx_membership_payments_athlete_month ON membership_payments(athlete_id, month)",
        "CREATE INDEX IF NOT EXISTS idx_membership_payments_status_created ON membership_payments(status, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_membership_payments_month_status ON membership_payments(month, status)",
        "CREATE TABLE IF NOT EXISTS gallery_photos (
            id TEXT PRIMARY KEY,
            image_url TEXT NOT NULL,
            media_type TEXT NOT NULL DEFAULT 'image',
            caption TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            published INTEGER NOT NULL DEFAULT 1,
            author_id TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS coach_comments (
            id TEXT PRIMARY KEY,
            target_type TEXT NOT NULL,
            target_id TEXT NOT NULL,
            body TEXT NOT NULL,
            author_user_id TEXT REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_coach_comments_target ON coach_comments(target_type, target_id, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS training_plans (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            title TEXT NOT NULL,
            goal TEXT,
            week_start TEXT NOT NULL,
            duration_weeks INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL DEFAULT 'planned',
            coach_note TEXT,
            athlete_note TEXT,
            progress_percent INTEGER NOT NULL DEFAULT 0,
            created_by TEXT REFERENCES users(id),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_training_plans_athlete_week ON training_plans(athlete_id, week_start DESC)",
        "CREATE TABLE IF NOT EXISTS exercises (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            category TEXT,
            description TEXT,
            video_url TEXT,
            created_at TEXT NOT NULL
        )",
        // Osobny system „Inne ćwiczenia” — zgłoszenia + historia (nie dotyka tabeli `results`)
        "CREATE TABLE IF NOT EXISTS exercise_submissions (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            exercise_id TEXT NOT NULL REFERENCES exercises(id) ON DELETE RESTRICT,
            value REAL NOT NULL,
            unit TEXT NOT NULL DEFAULT 'kg',
            performed_at TEXT NOT NULL,
            notes TEXT,
            status TEXT NOT NULL DEFAULT 'Pending',
            reviewed_by_user_id TEXT REFERENCES users(id),
            reviewed_at TEXT,
            review_note TEXT,
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_exercise_submissions_athlete_created ON exercise_submissions(athlete_id, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_exercise_submissions_status_created ON exercise_submissions(status, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_exercise_submissions_exercise_created ON exercise_submissions(exercise_id, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS exercise_results_history (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            exercise_id TEXT NOT NULL REFERENCES exercises(id) ON DELETE RESTRICT,
            value REAL NOT NULL,
            unit TEXT NOT NULL DEFAULT 'kg',
            performed_at TEXT NOT NULL,
            submission_id TEXT REFERENCES exercise_submissions(id) ON DELETE SET NULL,
            approved_by_user_id TEXT REFERENCES users(id),
            approved_at TEXT NOT NULL,
            notes TEXT,
            review_note TEXT,
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_exercise_history_exercise_value ON exercise_results_history(exercise_id, value DESC)",
        "CREATE INDEX IF NOT EXISTS idx_exercise_history_athlete_exercise ON exercise_results_history(athlete_id, exercise_id, value DESC)",
        "CREATE INDEX IF NOT EXISTS idx_exercise_history_created ON exercise_results_history(created_at DESC)",
        "CREATE TABLE IF NOT EXISTS recovery_logs (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id) ON DELETE CASCADE,
            date TEXT NOT NULL,
            sleep_hours REAL NOT NULL,
            fatigue_level INTEGER NOT NULL,
            soreness_level INTEGER NOT NULL,
            readiness_level INTEGER NOT NULL,
            note TEXT,
            created_at TEXT NOT NULL
        )",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_recovery_athlete_date ON recovery_logs(athlete_id, date)",
        "CREATE TABLE IF NOT EXISTS contact_messages (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            email TEXT,
            phone TEXT,
            message TEXT NOT NULL,
            created_at TEXT NOT NULL,
            is_read INTEGER NOT NULL DEFAULT 0
        )",
    ];

    for sql in create_tables {
        conn.execute(sql, ()).await?;
    }

    let _ = conn
        .execute(
            "ALTER TABLE competitions ADD COLUMN category TEXT DEFAULT 'club_event'",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE competitions ADD COLUMN status TEXT DEFAULT 'scheduled'",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE competitions ADD COLUMN category_override TEXT",
            (),
        )
        .await;
    let _ = conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_competitions_external_ref ON competitions(external_source, external_ref) WHERE external_source IS NOT NULL AND external_ref IS NOT NULL",
        (),
    )
    .await;
    let _ = conn
        .execute(
            "ALTER TABLE athletes ADD COLUMN is_active BOOLEAN DEFAULT 1",
            (),
        )
        .await;
    let _ = conn
        .execute(
            "UPDATE athletes SET is_active = 1 WHERE is_active IS NULL",
            (),
        )
        .await;
    let _ = conn
        .execute("ALTER TABLE athletes ADD COLUMN gender TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE athletes ADD COLUMN profile_tagline TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE athletes ADD COLUMN public_bio TEXT", ())
        .await;
    let _ = conn
        .execute(
            "ALTER TABLE athletes ADD COLUMN has_standing_order INTEGER NOT NULL DEFAULT 0",
            (),
        )
        .await;

    seed_data(conn).await?;
    slavia_info!("db.rs", "database rebuilt and seeded", "unset REBUILD_DB after local dev reset");
    Ok(())
}

async fn seed_data(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let argon2 = Argon2::default();
    let salt = SaltString::generate(&mut OsRng);

    // 1. Superadmin (Główny)
    let super_pass = "SLAVIA2026";
    let super_hash = argon2
        .hash_password(super_pass.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();
    let super_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, username, password_hash, roles) VALUES (?1, ?2, ?3, ?4)",
        (super_id.clone(), "Slavia", super_hash, "[\"SuperAdmin\"]"),
    )
    .await?;

    // 2. Jakub Gawron
    let jakub_pass = "Jakubzofia2030?";
    let jakub_hash = argon2
        .hash_password(jakub_pass.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();
    let jakub_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, username, password_hash, roles) VALUES (?1, ?2, ?3, ?4)",
        (
            jakub_id.clone(),
            "JakubGawron",
            jakub_hash,
            "[\"SuperAdmin\"]",
        ),
    )
    .await?;

    conn.execute(
        "INSERT INTO athletes (id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, profile_tagline, public_bio, is_active)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1)",
        (
            Uuid::new_v4().to_string(),
            Some(jakub_id),
            "Jakub Gawron",
            2000,
            "male",
            "Senior M — 85 kg",
            80.5,
            110.0,
            140.0,
            250.0,
            Some("https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/people/smiling-man".to_string()),
            "Założyciel klubu.",
            Some("Założyciel · trener".to_string()),
            Some("Założyciel sekcji i jeden z filarów rozwoju CKS Slavia.".to_string()),
        ),
    ).await?;

    // 3. Athletes seed with images (kategorie wagowe wg regulaminu PZPC od 1.01.2026)
    let athletes = [
        (
            "Anna Nowak",
            1998,
            "female",
            "Senior K — 69 kg",
            63.5,
            85.0,
            105.0,
            190.0,
            "https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/people/kitchen-bar",
            "Mistrzyni Polski",
        ),
        (
            "Piotr Zieliński",
            2002,
            "male",
            "Senior M — 110 kg",
            101.2,
            140.0,
            175.0,
            315.0,
            "https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/people/bicycle",
            "Rekordzista",
        ),
        (
            "Marek Przykładowy",
            2005,
            "male",
            "U23 M — 75 kg",
            72.8,
            90.0,
            115.0,
            205.0,
            "https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/people/boy-snow-hoodie",
            "Młodzieżowiec",
        ),
    ];

    for (name, year, gender, cat, bw, snatch, cj, total, img, note) in athletes {
        conn.execute(
            "INSERT INTO athletes (id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, profile_tagline, public_bio, is_active)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, NULL, 1)",
            (Uuid::new_v4().to_string(), name, year, gender, cat, bw, snatch, cj, total, Some(img.to_string()), note),
        ).await?;
    }

    // 4. Competitions & Results
    let comp_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO competitions (id, title, date, location, description, category) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (comp_id.clone(), "Mistrzostwa Śląska Seniorów", "2026-06-15", "Ruda Śląska", "Główne zawody.", "championship"),
    ).await?;

    let comp_id2 = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO competitions (id, title, date, location, description, category) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (comp_id2.clone(), "Liga Śląska — Runda 1", "2026-05-20", "Katowice", "Pierwsza runda ligi.", "league"),
    ).await?;

    conn.execute(
        "INSERT INTO competitions (id, title, date, location, description, category) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (Uuid::new_v4().to_string(), "Obóz Letni Slavia", "2026-07-10", "Wisła", "Zgrupowanie letnie.", "club_event"),
    ).await?;

    // 5. Posts
    conn.execute(
        "INSERT INTO posts (id, title, content, author_id, image_url, created_at, published) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
        (Uuid::new_v4().to_string(), "Nowa strona klubu!", "Witajcie w nowym systemie. Cieszcie się pięknym designem i nowymi funkcjami!", super_id, Some("https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/landscapes/nature-mountains".to_string()), "2026-05-01T09:00:00Z"),
    ).await?;

    let n = insert_default_exercises(conn).await?;
    slavia_info!("db.rs", "default exercise dictionary seeded during REBUILD_DB", "no action needed", inserted = n);

    Ok(())
}
