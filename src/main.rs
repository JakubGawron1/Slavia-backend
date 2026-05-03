use std::env;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use tokio::net::TcpListener;

use dotenvy::dotenv;
use slavia_backend::DatabaseBackend;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = dotenv();

    let (database, jwt_secret, c_name, c_key, c_secret) = load_config()?;

    match &database {
        DatabaseBackend::Local(p) => {
            println!("📂 Baza lokalna (SQLite): {}", p.display());
        }
        DatabaseBackend::Remote { url, .. } => {
            println!("☁️  Baza zdalna (Turso): {url}");
        }
    }

    let app = slavia_backend::create_app(database, jwt_secret, c_name, c_key, c_secret)
        .await
        .expect("Failed to create application");

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

fn load_secrets_table() -> Option<toml::Table> {
    let raw = std::fs::read_to_string(Path::new("Secrets.toml")).ok()?;
    toml::from_str(&raw).ok()
}

fn pick_cfg(
    secrets: Option<&toml::Table>,
    env_key: &'static str,
    secret_key: &str,
) -> Option<String> {
    env::var(env_key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            secrets?
                .get(secret_key)
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

fn load_database_backend(secrets: Option<&toml::Table>) -> Result<DatabaseBackend, String> {
    let mode = pick_cfg(secrets, "DATABASE_MODE", "DATABASE_MODE")
        .unwrap_or_else(|| "turso".to_string())
        .to_lowercase();

    if matches!(mode.as_str(), "local" | "sqlite" | "file") {
        let path_str = pick_cfg(secrets, "DATABASE_LOCAL_PATH", "DATABASE_LOCAL_PATH")
            .unwrap_or_else(|| ".local/slavia.db".to_string());
        return Ok(DatabaseBackend::Local(PathBuf::from(path_str)));
    }

    if mode != "turso" && mode != "remote" {
        return Err(format!(
            "Nieznany DATABASE_MODE={mode:?}. Użyj: local | sqlite | file | turso | remote"
        ));
    }

    let url = pick_cfg(secrets, "TURSO_DATABASE_URL", "TURSO_DATABASE_URL").ok_or_else(|| {
        "Brak TURSO_DATABASE_URL (tryb turso). Ustaw zmienną lub Secrets.toml — albo DATABASE_MODE=local dla SQLite.".to_string()
    })?;

    let token = pick_cfg(secrets, "TURSO_AUTH_TOKEN", "TURSO_AUTH_TOKEN").unwrap_or_default();

    Ok(DatabaseBackend::Remote {
        url,
        auth_token: token,
    })
}

fn load_config(
) -> Result<(DatabaseBackend, String, String, String, String), Box<dyn std::error::Error + Send + Sync>>
{
    let secrets = load_secrets_table();

    let database = load_database_backend(secrets.as_ref()).map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        e.into()
    })?;

    let jwt_secret =
        pick_cfg(secrets.as_ref(), "JWT_SECRET", "JWT_SECRET").unwrap_or_else(|| {
            "default_secret_for_dev_only".to_string()
        });

    let cn = pick_cfg(
        secrets.as_ref(),
        "CLOUDINARY_CLOUD_NAME",
        "CLOUDINARY_CLOUD_NAME",
    )
    .unwrap_or_default();
    let ck =
        pick_cfg(secrets.as_ref(), "CLOUDINARY_API_KEY", "CLOUDINARY_API_KEY").unwrap_or_default();
    let cs = pick_cfg(
        secrets.as_ref(),
        "CLOUDINARY_API_SECRET",
        "CLOUDINARY_API_SECRET",
    )
    .unwrap_or_default();

    Ok((database, jwt_secret, cn, ck, cs))
}
