use std::env;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use tokio::net::TcpListener;

use dotenvy::dotenv;
use slavia_backend::DatabaseBackend;

type AppConfig = (
    DatabaseBackend,
    String,
    String,
    String,
    String,
    String,
    String,
);
type AppError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let _ = dotenv();

    let (database, jwt_secret, c_name, c_key, c_secret, groq_key, groq_model) = load_config()?;

    match &database {
        DatabaseBackend::Local(p) => {
            println!("📂 Baza lokalna (SQLite): {}", p.display());
        }
        DatabaseBackend::Remote {
            url,
            replica_path: Some(path),
            ..
        } => {
            println!("☁️  Baza Turso (embedded replica): {url} → {}", path.display());
        }
        DatabaseBackend::Remote {
            url,
            replica_path: None,
            ..
        } => {
            println!("☁️  Baza Turso (HTTP remote): {url}");
        }
    }

    if groq_key.trim().is_empty() {
        println!("ℹ️  Trener AI (Groq): wyłączony — brak GROQ_API_KEY");
    } else {
        println!(
            "🤖 Trener AI (Groq): włączony, model {}",
            if groq_model.trim().is_empty() {
                "llama-3.1-70b-versatile"
            } else {
                groq_model.trim()
            }
        );
    }

    let app = slavia_backend::create_app(
        database,
        jwt_secret,
        c_name,
        c_key,
        c_secret,
        groq_key,
        groq_model,
    )
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

    let use_replica = pick_cfg(secrets, "TURSO_USE_REPLICA", "TURSO_USE_REPLICA")
        .unwrap_or_else(|| "true".to_string())
        .to_lowercase();
    let replica_enabled = !matches!(use_replica.as_str(), "0" | "false" | "no" | "off");

    let replica_path = if replica_enabled {
        Some(PathBuf::from(
            pick_cfg(secrets, "TURSO_REPLICA_PATH", "TURSO_REPLICA_PATH")
                .unwrap_or_else(|| ".local/slavia-replica.db".to_string()),
        ))
    } else {
        None
    };

    Ok(DatabaseBackend::Remote {
        url,
        auth_token: token,
        replica_path,
    })
}

fn load_config() -> Result<AppConfig, AppError> {
    let secrets = load_secrets_table();

    let database = load_database_backend(secrets.as_ref())
        .map_err(|e| -> AppError { e.into() })?;

    let jwt_secret = pick_cfg(secrets.as_ref(), "JWT_SECRET", "JWT_SECRET")
        .unwrap_or_else(|| "default_secret_for_dev_only".to_string());

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
    let groq_key =
        pick_cfg(secrets.as_ref(), "GROQ_API_KEY", "GROQ_API_KEY").unwrap_or_default();
    let groq_model =
        pick_cfg(secrets.as_ref(), "GROQ_MODEL", "GROQ_MODEL").unwrap_or_default();
    Ok((database, jwt_secret, cn, ck, cs, groq_key, groq_model))
}
