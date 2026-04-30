//! Lokalny serwer bez Shuttle — `cargo run --bin slavia-local` (frontend: `NUXT_PUBLIC_API_BASE_URL=http://127.0.0.1:8000`).
//! Na Windows włączony jest większy stos przez `.cargo/config.toml` (libsql + pierwsze zapytania).
//! Sekrety: zmienne `TURSO_DATABASE_URL`, `TURSO_AUTH_TOKEN`, `JWT_SECRET` albo `Secrets.toml` w katalogu projektu.

use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();

    let (db_url, db_token, jwt_secret) = load_config()?;

    let app = slavia_backend::create_app(&db_url, &db_token, jwt_secret).await?;

    let addr: std::net::SocketAddr = "127.0.0.1:8000".parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("Slavia-local: http://{addr}");

    axum::serve(listener, app).await?;

    Ok(())
}

fn load_config() -> Result<(String, String, String), Box<dyn std::error::Error + Send + Sync>> {
    let from_env = (
        std::env::var("TURSO_DATABASE_URL").ok(),
        std::env::var("TURSO_AUTH_TOKEN").ok(),
        std::env::var("JWT_SECRET").ok(),
    );

    if let (Some(url), token, jwt) = from_env {
        if !url.is_empty() {
            return Ok((
                url,
                token.unwrap_or_default(),
                jwt.unwrap_or_else(|| "default_secret_for_dev_only".to_string()),
            ));
        }
    }

    let path = Path::new("Secrets.toml");
    if !path.exists() {
        return Err(
            "Brak konfiguracji: ustaw TURSO_DATABASE_URL (i opcjonalnie TURSO_AUTH_TOKEN, JWT_SECRET) \
             albo umieść Secrets.toml w katalogu projektu."
                .into(),
        );
    }

    let raw = std::fs::read_to_string(path)?;
    let table: toml::Table = toml::from_str(&raw)?;
    let url = table
        .get("TURSO_DATABASE_URL")
        .and_then(|v| v.as_str())
        .ok_or("Secrets.toml: brak TURSO_DATABASE_URL")?
        .to_string();
    let token = table
        .get("TURSO_AUTH_TOKEN")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let jwt = table
        .get("JWT_SECRET")
        .and_then(|v| v.as_str())
        .unwrap_or("default_secret_for_dev_only")
        .to_string();

    Ok((url, token, jwt))
}
