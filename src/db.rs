use libsql::Connection;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use uuid::Uuid;

pub async fn init_db(
    conn: &Connection,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Pojedyncze `execute` zamiast `execute_batch`: na Windows + libsql/hrana `execute_batch`
    // potrafi przepełnić stos przy wielu instrukcjach w jednym żądaniu.
    const DDL: &[&str] = &[
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS athletes (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            birth_year INTEGER,
            weight_category TEXT,
            notes TEXT,
            is_active BOOLEAN DEFAULT 1
        )",
        "CREATE TABLE IF NOT EXISTS results (
            id TEXT PRIMARY KEY,
            athlete_id TEXT NOT NULL REFERENCES athletes(id),
            snatch REAL NOT NULL,
            clean_and_jerk REAL NOT NULL,
            total REAL NOT NULL,
            status TEXT NOT NULL,
            date TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS competitions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            date TEXT NOT NULL,
            location TEXT NOT NULL,
            description TEXT
        )",
        "CREATE TABLE IF NOT EXISTS posts (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            author_id TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL
        )",
    ];

    for sql in DDL {
        conn.execute(*sql, ()).await?;
    }

    // Check if any admin exists
    let mut rows = conn.query("SELECT COUNT(*) FROM users WHERE role = 'SuperAdmin'", ()).await?;
    let row = rows.next().await?.unwrap();
    let count: i64 = row.get(0)?;

    if count == 0 {
        // Create initial admins
        let argon2 = Argon2::default();
        let salt = SaltString::generate(&mut OsRng);
        
        let superadmin_pass = "superadmin123";
        let superadmin_hash = argon2.hash_password(superadmin_pass.as_bytes(), &salt).unwrap().to_string();

        let salt2 = SaltString::generate(&mut OsRng);
        let admin1_pass = "admin1_pass";
        let admin1_hash = argon2.hash_password(admin1_pass.as_bytes(), &salt2).unwrap().to_string();

        let salt3 = SaltString::generate(&mut OsRng);
        let admin2_pass = "admin2_pass";
        let admin2_hash = argon2.hash_password(admin2_pass.as_bytes(), &salt3).unwrap().to_string();

        let salt4 = SaltString::generate(&mut OsRng);
        let admin3_pass = "admin3_pass";
        let admin3_hash = argon2.hash_password(admin3_pass.as_bytes(), &salt4).unwrap().to_string();

        conn.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
            (Uuid::new_v4().to_string(), "superadmin", superadmin_hash, "SuperAdmin"),
        ).await?;

        conn.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
            (Uuid::new_v4().to_string(), "admin1", admin1_hash, "Admin"),
        ).await?;

        conn.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
            (Uuid::new_v4().to_string(), "admin2", admin2_hash, "Admin"),
        ).await?;

        conn.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
            (Uuid::new_v4().to_string(), "admin3", admin3_hash, "Admin"),
        ).await?;

        println!("Created default users: superadmin (superadmin123), admin1 (admin1_pass), admin2 (admin2_pass), admin3 (admin3_pass)");
    }

    // Check if Slavia superadmin exists
    let mut rows_slavia = conn.query("SELECT COUNT(*) FROM users WHERE username = 'Slavia'", ()).await?;
    let row_slavia = rows_slavia.next().await?.unwrap();
    let slavia_count: i64 = row_slavia.get(0)?;

    if slavia_count == 0 {
        let argon2 = Argon2::default();
        let salt = SaltString::generate(&mut OsRng);
        let slavia_hash = argon2.hash_password(b"SLAVIA2026", &salt).unwrap().to_string();
        
        conn.execute(
            "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
            (Uuid::new_v4().to_string(), "Slavia", slavia_hash, "SuperAdmin"),
        ).await?;
        println!("Created Slavia SuperAdmin!");
    }

    // Check if athletes exist
    let mut rows_athletes = conn.query("SELECT COUNT(*) FROM athletes", ()).await?;
    let row_athletes = rows_athletes.next().await?.unwrap();
    let athletes_count: i64 = row_athletes.get(0)?;

    if athletes_count == 0 {
        // Create 5 dummy users and athletes
        let argon2 = Argon2::default();
        let salt = SaltString::generate(&mut OsRng);
        let default_hash = argon2.hash_password(b"zawodnik123", &salt).unwrap().to_string();

        let dummy_data = [
            ("jan.kowalski", "Jan", "Kowalski", 1995, "81kg"),
            ("anna.nowak", "Anna", "Nowak", 1998, "64kg"),
            ("piotr.zielinski", "Piotr", "Zieliński", 2000, "102kg"),
            ("katarzyna.wojcik", "Katarzyna", "Wójcik", 1992, "71kg"),
            ("michal.lewandowski", "Michał", "Lewandowski", 1997, "89kg"),
        ];

        for (username, first_name, last_name, birth_year, weight_cat) in dummy_data {
            let user_id = Uuid::new_v4().to_string();
            let athlete_id = Uuid::new_v4().to_string();

            conn.execute(
                "INSERT INTO users (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
                (user_id.clone(), username, default_hash.clone(), "Athlete"),
            ).await?;

            conn.execute(
                "INSERT INTO athletes (id, user_id, first_name, last_name, birth_year, weight_category) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (athlete_id, user_id, first_name, last_name, birth_year, weight_cat),
            ).await?;
        }
        println!("Created 5 example athletes!");
    }

    Ok(())
}
