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
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use uuid::Uuid;

/// Najlepszy zatwierdzony start (max `total`, przy remisie nowsza `date`) → rekord zawodnika.
pub async fn sync_athlete_bests_from_approved_conn(
    conn: &Connection,
    athlete_id: &str,
) -> Result<(), libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT snatch, clean_and_jerk, total FROM results \
             WHERE athlete_id = ?1 AND status = 'Approved' \
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

/// Migracja przy starcie: wszystkie wiersze `athletes` — `best_*` wyłącznie z tabeli `results` (Approved).
pub async fn sync_all_athletes_bests_from_results(conn: &Connection) -> Result<u64, libsql::Error> {
    conn.execute(
        "UPDATE athletes SET \
           best_snatch_kg = ( \
             SELECT r.snatch FROM results r \
             WHERE r.athlete_id = athletes.id AND r.status = 'Approved' \
             ORDER BY r.total DESC, r.date DESC LIMIT 1 \
           ), \
           best_clean_jerk_kg = ( \
             SELECT r.clean_and_jerk FROM results r \
             WHERE r.athlete_id = athletes.id AND r.status = 'Approved' \
             ORDER BY r.total DESC, r.date DESC LIMIT 1 \
           ), \
           total_kg = ( \
             SELECT r.total FROM results r \
             WHERE r.athlete_id = athletes.id AND r.status = 'Approved' \
             ORDER BY r.total DESC, r.date DESC LIMIT 1 \
           )",
        (),
    )
    .await
}

pub async fn init_db(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Ustaw w `.env`: REBUILD_DB=true (jednorazowo, tylko dev), potem false — patrz `.env.example`
    let rebuild = std::env::var("REBUILD_DB").unwrap_or_default() == "true";
    
    if rebuild {
        println!("🧹 REBUILD_DB=true: Czyszczenie bazy danych...");
        reset_database(conn).await?;
        return Ok(());
    }

    // Tworzenie tabel if not exists
    let create_tables = [
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            email TEXT UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL,
            avatar_url TEXT
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
            is_active BOOLEAN DEFAULT 1
        )",
        "CREATE TABLE IF NOT EXISTS competitions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            date TEXT NOT NULL,
            location TEXT NOT NULL,
            description TEXT,
            category TEXT DEFAULT 'club_event',
            status TEXT DEFAULT 'scheduled',
            external_source TEXT,
            external_ref TEXT,
            external_url TEXT
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
            date TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS posts (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            author_id TEXT NOT NULL REFERENCES users(id),
            image_url TEXT,
            created_at TEXT NOT NULL
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
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_notifications_user_created ON notifications(user_id, created_at DESC)",
    ];

    for sql in create_tables {
        conn.execute(sql, ()).await?;
    }

    // Migrate: add category and status columns if missing (safe for existing DBs)
    let _ = conn.execute("ALTER TABLE competitions ADD COLUMN category TEXT DEFAULT 'club_event'", ()).await;
    let _ = conn.execute("ALTER TABLE competitions ADD COLUMN status TEXT DEFAULT 'scheduled'", ()).await;

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
        .execute("ALTER TABLE users ADD COLUMN avatar_url TEXT", ())
        .await;

    let _ = conn
        .execute("ALTER TABLE competitions ADD COLUMN external_source TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE competitions ADD COLUMN external_ref TEXT", ())
        .await;
    let _ = conn
        .execute("ALTER TABLE competitions ADD COLUMN external_url TEXT", ())
        .await;
    let _ = conn
        .execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_competitions_external_ref ON competitions(external_source, external_ref) WHERE external_source IS NOT NULL AND external_ref IS NOT NULL",
            (),
        )
        .await;

    let n = sync_all_athletes_bests_from_results(conn)
        .await
        .map_err(|e| format!("sync_all_athletes_bests_from_results: {e}"))?;
    println!(
        "📊 Migracja: athletes.best_* / total_kg ← najlepszy wynik Approved z `results` (sqlite changes={}).",
        n
    );

    Ok(())
}

pub async fn reset_database(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let drop_tables = [
        "DROP TABLE IF EXISTS notifications",
        "DROP TABLE IF EXISTS results",
        "DROP TABLE IF EXISTS competition_participants",
        "DROP TABLE IF EXISTS training_log_entries",
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
            email TEXT UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL,
            avatar_url TEXT
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
            is_active BOOLEAN DEFAULT 1
        )",
        "CREATE TABLE IF NOT EXISTS competitions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            date TEXT NOT NULL,
            location TEXT NOT NULL,
            description TEXT,
            category TEXT DEFAULT 'club_event',
            status TEXT DEFAULT 'scheduled',
            external_source TEXT,
            external_ref TEXT,
            external_url TEXT
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
            date TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS posts (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            author_id TEXT NOT NULL REFERENCES users(id),
            image_url TEXT,
            created_at TEXT NOT NULL
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
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_notifications_user_created ON notifications(user_id, created_at DESC)",
    ];

    for sql in create_tables {
        conn.execute(sql, ()).await?;
    }

    let _ = conn.execute("ALTER TABLE competitions ADD COLUMN category TEXT DEFAULT 'club_event'", ()).await;
    let _ = conn.execute("ALTER TABLE competitions ADD COLUMN status TEXT DEFAULT 'scheduled'", ()).await;
    let _ = conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_competitions_external_ref ON competitions(external_source, external_ref) WHERE external_source IS NOT NULL AND external_ref IS NOT NULL",
        (),
    )
    .await;
    let _ = conn.execute("ALTER TABLE athletes ADD COLUMN is_active BOOLEAN DEFAULT 1", ()).await;
    let _ = conn.execute("UPDATE athletes SET is_active = 1 WHERE is_active IS NULL", ()).await;
    let _ = conn.execute("ALTER TABLE athletes ADD COLUMN gender TEXT", ()).await;

    seed_data(conn).await?;
    println!("✅ Baza danych zrekonstruowana i zasilona danymi!");
    Ok(())
}

async fn seed_data(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let argon2 = Argon2::default();
    let salt = SaltString::generate(&mut OsRng);

    // 1. Superadmin (Główny)
    let super_pass = "SLAVIA2026";
    let super_hash = argon2.hash_password(super_pass.as_bytes(), &salt).map_err(|e| e.to_string())?.to_string();
    let super_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, username, email, password_hash, role) VALUES (?1, ?2, ?3, ?4, ?5)",
        (super_id.clone(), "Slavia", Some("biuro@slavia.pl".to_string()), super_hash, "SuperAdmin"),
    ).await?;

    // 2. Jakub Gawron
    let jakub_pass = "Jakubzofia2030?";
    let jakub_hash = argon2.hash_password(jakub_pass.as_bytes(), &salt).map_err(|e| e.to_string())?.to_string();
    let jakub_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, username, email, password_hash, role) VALUES (?1, ?2, ?3, ?4, ?5)",
        (jakub_id.clone(), "JakubGawron", Some("jakubgawron.dev.pl@gmail.com".to_string()), jakub_hash, "SuperAdmin"),
    ).await?;

    conn.execute(
        "INSERT INTO athletes (id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, is_active)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1)",
        (Uuid::new_v4().to_string(), Some(jakub_id), "Jakub Gawron", 2000, "male", "81kg", 80.5, 110.0, 140.0, 250.0, Some("https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/people/smiling-man".to_string()), "Założyciel klubu.",),
    ).await?;

    // 3. Athletes seed with images
    let athletes = [
        ("Anna Nowak", 1998, "female", "64kg", 63.5, 85.0, 105.0, 190.0, "https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/people/kitchen-bar", "Mistrzyni Polski"),
        ("Piotr Zieliński", 2002, "male", "102kg", 101.2, 140.0, 175.0, 315.0, "https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/people/bicycle", "Rekordzista"),
        ("Marek Przykładowy", 2005, "male", "73kg", 72.8, 90.0, 115.0, 205.0, "https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/people/boy-snow-hoodie", "Junior"),
    ];

    for (name, year, gender, cat, bw, snatch, cj, total, img, note) in athletes {
        conn.execute(
            "INSERT INTO athletes (id, user_id, full_name, birth_year, gender, weight_category, bodyweight, best_snatch_kg, best_clean_jerk_kg, total_kg, image_url, notes, is_active)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1)",
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
        "INSERT INTO posts (id, title, content, author_id, image_url, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (Uuid::new_v4().to_string(), "Nowa strona klubu!", "Witajcie w nowym systemie. Cieszcie się pięknym designem i nowymi funkcjami!", super_id, Some("https://res.cloudinary.com/dbm5i0jad/image/upload/v1/samples/landscapes/nature-mountains".to_string()), "2026-05-01T09:00:00Z"),
    ).await?;

    Ok(())
}
