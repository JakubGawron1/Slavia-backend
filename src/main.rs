use std::env;
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpListener;
use dotenvy::dotenv;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Próbujemy załadować .env
    let _ = dotenv();

    // Ładujemy konfigurację (Env > Secrets.toml)
    let (db_url, db_token, jwt_secret) = load_config()?;

    let app = slavia_backend::create_app(&db_url, &db_token, jwt_secret)
        .await
        .expect("Failed to create application");

    // Port z env (np. dla Render/Railways) lub domyślnie 8080
    let port = env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .expect("PORT must be a number");

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("🚀 Serwer Slavia-backend startuje na http://{}", addr);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn load_config() -> Result<(String, String, String), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Próbujemy z zmiennych środowiskowych
    let from_env = (
        env::var("TURSO_DATABASE_URL").ok(),
        env::var("TURSO_AUTH_TOKEN").ok(),
        env::var("JWT_SECRET").ok(),
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

    // 2. Próbujemy z Secrets.toml (pozostałość po Shuttle/slavia-local)
    let path = Path::new("Secrets.toml");
    if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        let table: toml::Table = toml::from_str(&raw)?;
        
        let url = table.get("TURSO_DATABASE_URL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
            
        let token = table.get("TURSO_AUTH_TOKEN")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
            
        let jwt = table.get("JWT_SECRET")
            .and_then(|v| v.as_str())
            .unwrap_or("default_secret_for_dev_only")
            .to_string();

        if let Some(url) = url {
            return Ok((url, token, jwt));
        }
    }

    Err("Brak konfiguracji! Ustaw zmienne środowiskowe (TURSO_DATABASE_URL) lub plik Secrets.toml / .env".into())
}
