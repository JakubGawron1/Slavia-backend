//! Guardy produkcyjne - wspólna detekcja środowiska Turso/prod i walidacja sekretów.

/// Domyślny JWT używany tylko w lokalnym dev (gdy brak `JWT_SECRET`).
pub const DEV_JWT_FALLBACK: &str = "default_secret_for_dev_only";

/// Minimalna długość `JWT_SECRET` przy zdalnej bazie (Turso).
pub const MIN_JWT_SECRET_LEN_PRODUCTION: usize = 32;

/// Czy konfiguracja wskazuje na zdalną produkcyjną bazę (Turso).
pub fn remote_database_configured() -> bool {
    let mode = std::env::var("DATABASE_MODE")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(mode.as_str(), "turso" | "remote") {
        return true;
    }
    std::env::var("TURSO_DATABASE_URL")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

/// Blokuje destrukcyjne operacje (reset DB, backup Cloudinary) na Turso/produkcji.
pub fn destructive_db_ops_blocked() -> bool {
    remote_database_configured()
}

/// Cloudinary DB backup dozwolony tylko lokalnie lub po jawnej zgodzie (`ALLOW_CLOUDINARY_DB_BACKUP=1`).
pub fn cloudinary_db_backup_allowed() -> bool {
    if std::env::var("ALLOW_CLOUDINARY_DB_BACKUP")
        .map(|v| {
            let t = v.trim().to_ascii_lowercase();
            t == "1" || t == "true" || t == "yes"
        })
        .unwrap_or(false)
    {
        return true;
    }
    !remote_database_configured()
}

/// Odrzuca start z domyślnym lub zbyt krótkim JWT przy Turso.
pub fn validate_jwt_secret_for_startup(jwt_secret: &str) -> Result<(), String> {
    if !remote_database_configured() {
        return Ok(());
    }
    let s = jwt_secret.trim();
    if s.is_empty() || s == DEV_JWT_FALLBACK {
        return Err(
            "JWT_SECRET must be set to a strong value when using Turso/production database"
                .to_string(),
        );
    }
    if s.len() < MIN_JWT_SECRET_LEN_PRODUCTION {
        return Err(format!(
            "JWT_SECRET must be at least {MIN_JWT_SECRET_LEN_PRODUCTION} characters on production"
        ));
    }
    Ok(())
}

/// Ścieżki API, dla których nigdy nie logujemy treści żądań (RODO / Trener AI).
pub fn is_ai_content_path(path: &str) -> bool {
    path.starts_with("/api/ai/coach")
        || path.starts_with("/api/ai/barbell")
        || path.contains("/chat")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn with_envs(pairs: &[(&str, Option<&str>)], f: impl FnOnce()) {
        let _guard = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev: Vec<(&str, Option<String>)> = pairs
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        unsafe {
            for (k, v) in pairs {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        f();
        unsafe {
            for (k, prev_v) in prev {
                match prev_v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }


    #[test]
    fn remote_database_detected_from_mode() {
        with_envs(
            &[("DATABASE_MODE", Some("turso")), ("TURSO_DATABASE_URL", None)],
            || {
                assert!(remote_database_configured());
            },
        );
    }

    #[test]
    fn remote_database_detected_from_url() {
        with_envs(
            &[
                ("DATABASE_MODE", Some("local")),
                ("TURSO_DATABASE_URL", Some("libsql://example")),
            ],
            || {
                assert!(remote_database_configured());
            },
        );
    }

    #[test]
    fn jwt_rejects_default_on_production() {
        with_envs(&[("DATABASE_MODE", Some("turso"))], || {
            let err = validate_jwt_secret_for_startup(DEV_JWT_FALLBACK).unwrap_err();
            assert!(err.contains("JWT_SECRET"));
        });
    }

    #[test]
    fn jwt_allows_default_on_local() {
        with_envs(
            &[
                ("DATABASE_MODE", Some("local")),
                ("TURSO_DATABASE_URL", None),
            ],
            || {
                assert!(validate_jwt_secret_for_startup(DEV_JWT_FALLBACK).is_ok());
            },
        );
    }

    #[test]
    fn cloudinary_backup_blocked_without_opt_in_on_turso() {
        with_envs(
            &[
                ("DATABASE_MODE", Some("turso")),
                ("ALLOW_CLOUDINARY_DB_BACKUP", None),
            ],
            || {
                assert!(!cloudinary_db_backup_allowed());
            },
        );
    }

    #[test]
    fn ai_paths_detected() {
        assert!(is_ai_content_path("/api/ai/coach/chat"));
        assert!(!is_ai_content_path("/api/system/ping"));
    }
}
