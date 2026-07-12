use std::path::Path;
use std::time::Duration;

use deadpool_libsql::{Manager, Pool, Runtime};
use libsql::Connection;

use crate::DatabaseBackend;

/// Błędy chwilowe libsql/SQLite (lock, zerwana sesja HTTP z Turso).
fn is_transient_db_error(e: &libsql::Error) -> bool {
    let msg = e.to_string().to_ascii_lowercase();
    msg.contains("stream not found")
        || msg.contains("database is locked")
        || msg.contains("database table is locked")
        || msg.contains("sqlite_busy")
        || msg.contains("sqlite_locked")
        || crate::db::sqlite_corrupt_message(&msg)
}

const DB_TRANSIENT_MAX_ATTEMPTS: u32 = 4;

async fn with_db_retry<T, F, Fut>(label: &str, mut op: F) -> Result<T, libsql::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, libsql::Error>>,
{
    let mut last_err: Option<libsql::Error> = None;
    for attempt in 1..=DB_TRANSIENT_MAX_ATTEMPTS {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) if is_transient_db_error(&e) && attempt < DB_TRANSIENT_MAX_ATTEMPTS => {
                slavia_warn!("state.rs", "transient database error", "automatic retry with backoff", error = %e, attempt, op = label);
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(30 * attempt as u64)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("retry loop exhausted without error"))
}

fn pool_size(backend: &DatabaseBackend) -> usize {
    let configured = std::env::var("DATABASE_POOL_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| (1..=64).contains(&n));
    let base = configured.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| (n.get() * 2).clamp(4, 16))
            .unwrap_or(8)
    });
    // Lokalny SQLite = jeden plik; zbyt duża pula = locki.
    if configured.is_none() && matches!(backend, DatabaseBackend::Local(_)) {
        base.min(6)
    } else {
        base
    }
}

/// Połączenie z poola — zwraca się do puli po drop.
pub struct PooledConn(deadpool_libsql::Object);

impl AsRef<Connection> for PooledConn {
    fn as_ref(&self) -> &Connection {
        &self.0
    }
}

impl std::ops::Deref for PooledConn {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PooledConn {
    pub async fn query<P>(&self, sql: &str, params: P) -> Result<libsql::Rows, libsql::Error>
    where
        P: libsql::params::IntoParams + Clone + Send,
    {
        let sql = sql.to_string();
        with_db_retry("query", || {
            let sql = sql.clone();
            let params = params.clone();
            async move { Connection::query(self.as_ref(), &sql, params).await }
        })
        .await
    }

    pub async fn execute<P>(&self, sql: &str, params: P) -> Result<u64, libsql::Error>
    where
        P: libsql::params::IntoParams + Clone + Send,
    {
        let sql = sql.to_string();
        with_db_retry("execute", || {
            let sql = sql.clone();
            let params = params.clone();
            async move { Connection::execute(self.as_ref(), &sql, params).await }
        })
        .await
    }
}

#[derive(Clone)]
pub struct Db {
    pool: Pool,
    backend: DatabaseBackend,
}

impl Db {
    /// Otwiera pulę, uruchamia `init_db`; przy uszkodzonym pliku SQLite jednorazowo kasuje pliki i ponawia.
    pub async fn open_with_migrations(
        backend: DatabaseBackend,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut wiped_for_migrations = false;
        loop {
            let db = Self::new(backend.clone()).await?;
            let init_conn = db.raw().await.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                e.into()
            })?;
            let _ = apply_connection_pragmas(&init_conn).await;
            match crate::db::init_db(init_conn.as_ref()).await {
                Ok(()) => return Ok(db),
                Err(e)
                    if crate::db::error_indicates_sqlite_corrupt(e.as_ref())
                        && !wiped_for_migrations
                        && let Some(path) = backend.local_sqlite_path() =>
                {
                    slavia_warn!(
                        "state.rs",
                        "SQLite file is corrupt during init_db",
                        "local files were wiped and startup will retry",
                        path = %path.display(),
                        error = %e
                    );
                    wipe_local_sqlite_files(path);
                    wiped_for_migrations = true;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub async fn new(
        backend: DatabaseBackend,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut sqlite_wiped = false;
        let database = loop {
            if let Some(path) = backend.local_sqlite_path() {
                maybe_wipe_invalid_sqlite_header(path);
            }

            let database = match build_database(&backend).await {
                Ok(db) => db,
                Err(e)
                    if crate::db::sqlite_corrupt_message(&e.to_string())
                        && !sqlite_wiped
                        && let Some(path) = backend.local_sqlite_path() =>
                {
                    slavia_warn!(
                        "state.rs",
                        "SQLite file is corrupt during build_database",
                        "local files were wiped and connection will retry",
                        path = %path.display(),
                        error = %e
                    );
                    wipe_local_sqlite_files(path);
                    sqlite_wiped = true;
                    continue;
                }
                Err(e) => return Err(e),
            };

            if let Some(path) = backend.local_sqlite_path() {
                match database.connect() {
                    Ok(conn) => {
                        let integrity = check_sqlite_integrity(&conn).await;
                        drop(conn);

                        if matches!(&integrity, Ok(true)) {
                            break database;
                        }

                        let err_hint = match &integrity {
                            Ok(false) => "integrity_check != ok".to_string(),
                            Err(e) => e.to_string(),
                            Ok(true) => String::new(),
                        };
                        if !sqlite_wiped {
                            slavia_warn!(
                                "state.rs",
                                "SQLite integrity_check failed",
                                "local database files were wiped and startup will retry",
                                path = %path.display(),
                                error = %err_hint
                            );
                            wipe_local_sqlite_files(path);
                            sqlite_wiped = true;
                            continue;
                        }

                        slavia_error!(
                            "state.rs",
                            "SQLite still corrupt after wipe",
                            "delete .local/slavia.db manually or set REBUILD_DB=true locally",
                            path = %path.display(),
                            error = %err_hint
                        );
                        break database;
                    }
                    Err(e)
                        if crate::db::sqlite_corrupt_message(&e.to_string()) && !sqlite_wiped =>
                    {
                        slavia_warn!(
                            "state.rs",
                            "file is not a valid SQLite database",
                            "invalid local files were wiped and connection will retry",
                            path = %path.display(),
                            error = %e
                        );
                        wipe_local_sqlite_files(path);
                        sqlite_wiped = true;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            } else {
                break database;
            }
        };

        if matches!(backend, DatabaseBackend::Local(_))
            && let Ok(conn) = database.connect()
        {
            let _ = apply_connection_pragmas(&conn).await;
        }

        let effective_pool_size = pool_size(&backend);
        let manager = Manager::from_libsql_database(database);
        let pool = Pool::builder(manager)
            .max_size(effective_pool_size)
            .runtime(Runtime::Tokio1)
            .build()?;

        slavia_info!(
            "state.rs",
            "database connection pool initialized",
            "tune DATABASE_POOL_SIZE if you see pool busy warnings",
            pool_size = effective_pool_size,
            mode = backend.describe()
        );

        Ok(Self { pool, backend })
    }

    pub fn backend(&self) -> &DatabaseBackend {
        &self.backend
    }

    /// Pożyczka połączenia z puli (zwraca się automatycznie po drop).
    pub async fn raw(&self) -> Result<PooledConn, deadpool_libsql::PoolError> {
        const MAX_ATTEMPTS: u32 = 5;
        let mut last_err: Option<deadpool_libsql::PoolError> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            match self.pool.get().await {
                Ok(c) => return Ok(PooledConn(c)),
                Err(e) => {
                    slavia_warn!("state.rs", "database pool checkout timed out", "increase DATABASE_POOL_SIZE or reduce concurrent load", error = %e, attempt);
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(25 * attempt as u64)).await;
                }
            }
        }
        Err(last_err.expect("pool retry loop exhausted without error"))
    }

    /// Best-effort połączenie (np. powiadomienia w tle) — bez panic.
    pub async fn raw_or_none(&self) -> Option<PooledConn> {
        match self.raw().await {
            Ok(c) => Some(c),
            Err(e) => {
                slavia_warn!("state.rs", "database pool unavailable", "retry later or check Turso/HF Space health", error = %e);
                None
            }
        }
    }

    pub async fn execute<P>(&self, sql: &str, params: P) -> Result<u64, libsql::Error>
    where
        P: libsql::params::IntoParams + Clone + Send,
    {
        let conn = self.raw().await.map_err(pool_as_libsql_err)?;
        conn.execute(sql, params).await
    }

    pub async fn query<P>(&self, sql: &str, params: P) -> Result<libsql::Rows, libsql::Error>
    where
        P: libsql::params::IntoParams + Clone + Send,
    {
        let conn = self.raw().await.map_err(pool_as_libsql_err)?;
        conn.query(sql, params).await
    }
}

pub(crate) fn pool_as_libsql_err(e: deadpool_libsql::PoolError) -> libsql::Error {
    libsql::Error::SqliteFailure(0, format!("pool unavailable: {e}"))
}

async fn check_sqlite_integrity(conn: &Connection) -> Result<bool, libsql::Error> {
    let mut rows = conn.query("PRAGMA integrity_check", ()).await?;
    let Some(row) = rows.next().await? else {
        return Ok(false);
    };
    let status = crate::sql_row::flex_string(&row, 0)?;
    Ok(status.eq_ignore_ascii_case("ok"))
}

/// Plik istnieje, ale nie zaczyna się od magicznego nagłówka SQLite.
fn local_sqlite_header_invalid(path: &Path) -> bool {
    use std::io::Read;

    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() == 0 {
        return false;
    }
    if meta.len() < 16 {
        return true;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 16];
    if file.read_exact(&mut header).is_err() {
        return true;
    }
    &header[..15] != b"SQLite format 3"
}

fn maybe_wipe_invalid_sqlite_header(path: &Path) {
    if path.exists() && local_sqlite_header_invalid(path) {
        slavia_warn!(
            "state.rs",
            "SQLite file has invalid header",
            "file was removed before open; fresh DB will be created",
            path = %path.display()
        );
        wipe_local_sqlite_files(path);
    }
}

/// Usuwa plik SQLite i towarzyszące `-wal` / `-shm`.
pub(crate) fn wipe_local_sqlite_files(path: &Path) {
    let base = path.to_string_lossy();
    for candidate in [
        base.to_string(),
        format!("{base}-wal"),
        format!("{base}-shm"),
    ] {
        if let Err(e) = std::fs::remove_file(&candidate)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            slavia_warn!("state.rs", "failed to delete SQLite sidecar file", "stop other backend processes and remove file manually", file = %candidate, error = %e);
        }
    }
}

async fn build_database(
    backend: &DatabaseBackend,
) -> Result<libsql::Database, Box<dyn std::error::Error + Send + Sync>> {
    match backend {
        DatabaseBackend::Local(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            Ok(libsql::Builder::new_local(path).build().await?)
        }
        DatabaseBackend::Remote { url, auth_token } => Ok(
            libsql::Builder::new_remote(url.clone(), auth_token.clone())
                .build()
                .await?,
        ),
    }
}

/// PRAGMA dla lokalnego SQLite — WAL + większy cache.
pub async fn apply_connection_pragmas(conn: &Connection) -> Result<(), libsql::Error> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA cache_size=-64000;
         PRAGMA temp_store=MEMORY;
         PRAGMA mmap_size=268435456;
         PRAGMA busy_timeout=5000;",
    )
    .await?;
    Ok(())
}

impl DatabaseBackend {
    pub fn local_sqlite_path(&self) -> Option<&Path> {
        match self {
            DatabaseBackend::Local(path) => Some(path.as_path()),
            DatabaseBackend::Remote { .. } => None,
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            DatabaseBackend::Local(_) => "local",
            DatabaseBackend::Remote { .. } => "turso",
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub jwt_secret: String,
    pub cloudinary_cloud_name: String,
    pub cloudinary_api_key: String,
    pub cloudinary_api_secret: String,
    pub groq_api_key: String,
    pub groq_model: String,
    pub worker_metrics: std::sync::Arc<crate::worker_metrics::WorkerMetrics>,
    pub http_metrics: std::sync::Arc<crate::http_metrics::HttpMetrics>,
    pub prometheus_metrics_enabled: bool,
}

impl AppState {
    pub async fn db_conn(&self) -> Result<PooledConn, crate::api_error::ApiError> {
        self.db.raw().await.map_err(crate::api_error::map_pool_err)
    }
}
