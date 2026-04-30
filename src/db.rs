use libsql::Connection;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use uuid::Uuid;

pub async fn init_db(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("🧹 Resetowanie i inicjalizacja bazy danych...");

    // Usuwamy stare tabele w poprawnej kolejności (klucze obce)
    let drop_tables = [
        "DROP TABLE IF EXISTS results",
        "DROP TABLE IF EXISTS posts",
        "DROP TABLE IF EXISTS competitions",
        "DROP TABLE IF EXISTS athletes",
        "DROP TABLE IF EXISTS users",
    ];

    for sql in drop_tables {
        let _ = conn.execute(sql, ()).await;
    }

    // Tworzenie tabel od zera
    let create_tables = [
        "CREATE TABLE users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL
        )",
        "CREATE TABLE athletes (
            id TEXT PRIMARY KEY,
            user_id TEXT REFERENCES users(id),
            full_name TEXT NOT NULL,
            birth_year INTEGER,
            weight_category TEXT,
            best_snatch_kg REAL,
            best_clean_jerk_kg REAL,
            total_kg REAL,
            notes TEXT,
            is_active BOOLEAN DEFAULT 1
        )",
        "CREATE TABLE results (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id),
            snatch REAL NOT NULL,
            clean_and_jerk REAL NOT NULL,
            total REAL NOT NULL,
            status TEXT NOT NULL,
            date TEXT NOT NULL
        )",
        "CREATE TABLE competitions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            date TEXT NOT NULL,
            location TEXT NOT NULL,
            description TEXT
        )",
        "CREATE TABLE posts (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            author_id TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
    ];

    for sql in create_tables {
        conn.execute(sql, ()).await?;
    }

    // Seedowanie danych
    seed_data(conn).await?;

    println!("✅ Baza danych zainicjalizowana pomyślnie!");
    Ok(())
}

async fn seed_data(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let argon2 = Argon2::default();
    let salt = SaltString::generate(&mut OsRng);

    // 1. Superadmin
    let super_pass = "SLAVIA2026";
    let super_hash = argon2.hash_password(super_pass.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();
    let super_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
        (super_id.clone(), "Slavia", super_hash, "SuperAdmin"),
    ).await?;

    // 2. Admins (4)
    let admin_pass = "admin123";
    let admin_logins = ["admin1", "admin2", "trener.kowalski", "kierownik.biura"];
    for login in admin_logins {
        let hash = argon2.hash_password(admin_pass.as_bytes(), &salt)
            .map_err(|e| e.to_string())?
            .to_string();
        conn.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
            (Uuid::new_v4().to_string(), login, hash, "Admin"),
        ).await?;
    }

    // 3. Athletes (5)
    let athletes = [
        ("Jan Kowalski", 1995, "81kg", 120.0, 150.0, 270.0, "Najlepszy zawodnik senior"),
        ("Anna Nowak", 1998, "64kg", 80.0, 100.0, 180.0, "Mistrzyni Polski Juniorek"),
        ("Piotr Zieliński", 2002, "102kg", 135.0, 165.0, 300.0, "Obiecujący zawodnik wagi ciężkiej"),
        ("Katarzyna Wójcik", 2000, "71kg", 85.0, 110.0, 195.0, "Stabilna forma, dobra technika"),
        ("Michał Lewandowski", 1992, "89kg", 115.0, 145.0, 260.0, "Weteran klubu, trener młodzieży"),
    ];

    for (name, year, cat, snatch, cj, total, note) in athletes {
        conn.execute(
            "INSERT INTO athletes (id, user_id, full_name, birth_year, weight_category, best_snatch_kg, best_clean_jerk_kg, total_kg, notes, is_active)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
            (Uuid::new_v4().to_string(), name, year, cat, snatch, cj, total, note),
        ).await?;
    }

    // 4. Competitions (2)
    conn.execute(
        "INSERT INTO competitions (id, title, date, location, description) VALUES (?1, ?2, ?3, ?4, ?5)",
        (Uuid::new_v4().to_string(), "Mistrzostwa Śląska Seniorów", "2026-06-15", "Ruda Śląska", "Główne zawody sezonu dla naszych seniorów."),
    ).await?;
    conn.execute(
        "INSERT INTO competitions (id, title, date, location, description) VALUES (?1, ?2, ?3, ?4, ?5)",
        (Uuid::new_v4().to_string(), "Puchar Polski Juniorów", "2026-09-20", "Warszawa", "Kwalifikacje do kadry narodowej."),
    ).await?;

    // 5. Posts (2)
    conn.execute(
        "INSERT INTO posts (id, title, content, author_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        (Uuid::new_v4().to_string(), "Sukces na Mistrzostwach!", "Nasi zawodnicy zdobyli 3 złote medale na ostatnich zawodach w Katowicach.", super_id.clone(), "2026-04-30T10:00:00Z"),
    ).await?;
    conn.execute(
        "INSERT INTO posts (id, title, content, author_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        (Uuid::new_v4().to_string(), "Nowe godziny treningów", "Od maja treningi grupy początkującej będą odbywać się o 17:00.", super_id, "2026-04-30T12:00:00Z"),
    ).await?;

    Ok(())
}
