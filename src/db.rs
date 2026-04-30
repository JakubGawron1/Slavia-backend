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
            last_name TEXT NOT NULL
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

    Ok(())
}
