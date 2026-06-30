use std::path::Path;
use std::time::Duration;

use deadpool_libsql::{Manager, Pool, Runtime};
use libsql::Connection;

use crate::DatabaseBackend;

fn is_stream_not_found_error(e: &libsql::Error) -> bool {
    let msg = e.to_string().to_ascii_lowercase();
    msg.contains("stream not found")
}

fn pool_size() -> usize {
    std::env::var("DATABASE_POOL_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| (1..=64).contains(&n))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * 2).clamp(4, 16))
                .unwrap_or(8)
        })
}

fn replica_sync_interval_secs() -> u64 {
    std::env::var("TURSO_REPLICA_SYNC_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60)
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

#[derive(Clone)]
pub struct Db {
    pool: Pool,
    backend: DatabaseBackend,
}

impl Db {
    /// Otwiera pulę, uruchamia `init_db`; przy uszkodzonej replice (malformed) jednorazowo kasuje pliki SQLite i ponawia.
    pub async fn open_with_migrations(
        backend: DatabaseBackend,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut wiped_for_migrations = false;
        loop {
            let db = Self::new(backend.clone()).await?;
            let init_conn = db.raw().await;
            let _ = apply_connection_pragmas(&init_conn).await;
            match crate::db::init_db(init_conn.as_ref()).await {
                Ok(()) => return Ok(db),
                Err(e)
                    if crate::db::error_indicates_sqlite_corrupt(e.as_ref())
                        && !wiped_for_migrations
                        && let Some(path) = backend.local_sqlite_path() =>
                {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "init_db: uszkodzona replika SQLite — usuwam pliki i ponawiam start"
                    );
                    wipe_local_sqlite_replica(path);
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
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "build_database: uszkodzony plik SQLite — usuwam i ponawiam sync z Turso"
                    );
                    wipe_local_sqlite_replica(path);
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
                            tracing::warn!(
                                path = %path.display(),
                                error = %err_hint,
                                "SQLite integrity_check nie powiódł się — usuwam lokalną kopię i ponawiam sync z Turso"
                            );
                            wipe_local_sqlite_replica(path);
                            sqlite_wiped = true;
                            continue;
                        }

                        tracing::error!(
                            path = %path.display(),
                            error = %err_hint,
                            "SQLite nadal uszkodzony po wipe — kontynuuję start (migracje best-effort)"
                        );
                        break database;
                    }
                    Err(e)
                        if crate::db::sqlite_corrupt_message(&e.to_string()) && !sqlite_wiped =>
                    {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "connect: plik nie jest bazą SQLite — usuwam i ponawiam sync z Turso"
                        );
                        wipe_local_sqlite_replica(path);
                        sqlite_wiped = true;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            } else {
                break database;
            }
        };

        if backend.uses_local_sqlite_engine()
            && let Ok(conn) = database.connect()
        {
            let _ = apply_connection_pragmas(&conn).await;
        }

        let manager = Manager::from_libsql_database(database);
        let pool = Pool::builder(manager)
            .max_size(pool_size())
            .runtime(Runtime::Tokio1)
            .build()?;

        tracing::info!(
            pool_size = pool_size(),
            mode = backend.describe(),
            "database pool ready"
        );

        Ok(Self { pool, backend })
    }

    pub fn backend(&self) -> &DatabaseBackend {
        &self.backend
    }

    /// Pożyczka połączenia z puli (zwraca się automatycznie po drop).
    pub async fn raw(&self) -> PooledConn {
        const MAX_ATTEMPTS: u32 = 5;
        let mut last_err: Option<deadpool_libsql::PoolError> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            match self.pool.get().await {
                Ok(c) => return PooledConn(c),
                Err(e) => {
                    tracing::warn!(error = %e, attempt, "database pool busy — retry");
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(25 * attempt as u64)).await;
                }
            }
        }
        panic!(
            "database pool unavailable after {MAX_ATTEMPTS} attempts: {:?}",
            last_err
        );
    }

    pub async fn execute<P>(&self, sql: &str, params: P) -> Result<u64, libsql::Error>
    where
        P: libsql::params::IntoParams + Clone + Send,
    {
        let conn = self.raw().await;
        match conn.execute(sql, params.clone()).await {
            Ok(v) => Ok(v),
            Err(e) if is_stream_not_found_error(&e) => {
                tracing::warn!(error = %e, "db execute: stream not found — retry");
                let conn2 = self.raw().await;
                conn2.execute(sql, params).await
            }
            Err(e) => Err(e),
        }
    }

    pub async fn query<P>(&self, sql: &str, params: P) -> Result<libsql::Rows, libsql::Error>
    where
        P: libsql::params::IntoParams + Clone + Send,
    {
        let conn = self.raw().await;
        match conn.query(sql, params.clone()).await {
            Ok(v) => Ok(v),
            Err(e) if is_stream_not_found_error(&e) => {
                tracing::warn!(error = %e, "db query: stream not found — retry");
                let conn2 = self.raw().await;
                conn2.query(sql, params).await
            }
            Err(e) => Err(e),
        }
    }
}

async fn check_sqlite_integrity(conn: &Connection) -> Result<bool, libsql::Error> {
    let mut rows = conn.query("PRAGMA integrity_check", ()).await?;
    let Some(row) = rows.next().await? else {
        return Ok(false);
    };
    let status = crate::sql_row::flex_string(&row, 0)?;
    Ok(status.eq_ignore_ascii_case("ok"))
}

/// Plik istnieje, ale nie zaczyna się od magicznego nagłówka SQLite (np. przerwany sync na Render).
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
        tracing::warn!(
            path = %path.display(),
            "Plik SQLite bez poprawnego nagłówka — usuwam przed otwarciem repliki"
        );
        wipe_local_sqlite_replica(path);
    }
}

/// Usuwa plik SQLite i towarzyszące `-wal` / `-shm` (np. uszkodzona replika Turso na Render).
pub(crate) fn wipe_local_sqlite_replica(path: &Path) {
    let base = path.to_string_lossy();
    for candidate in [
        base.to_string(),
        format!("{base}-wal"),
        format!("{base}-shm"),
    ] {
        if let Err(e) = std::fs::remove_file(&candidate)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(file = %candidate, error = %e, "nie udało się usunąć pliku SQLite");
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
        DatabaseBackend::Remote {
            url,
            auth_token,
            replica_path,
        } => {
            if let Some(path) = replica_path {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                Ok(libsql::Builder::new_remote_replica(
                    path,
                    url.clone(),
                    auth_token.clone(),
                )
                .sync_interval(Duration::from_secs(replica_sync_interval_secs()))
                .read_your_writes(true)
                .build()
                .await?)
            } else {
                Ok(libsql::Builder::new_remote(url.clone(), auth_token.clone())
                    .build()
                    .await?)
            }
        }
    }
}

/// PRAGMA dla lokalnego SQLite / embedded replica — WAL + większy cache.
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
            DatabaseBackend::Remote {
                replica_path: Some(path),
                ..
            } => Some(path.as_path()),
            DatabaseBackend::Remote {
                replica_path: None, ..
            } => None,
        }
    }

    pub fn uses_local_sqlite_engine(&self) -> bool {
        matches!(self, DatabaseBackend::Local(_) | DatabaseBackend::Remote { replica_path: Some(_), .. })
    }

    pub fn describe(&self) -> &'static str {
        match self {
            DatabaseBackend::Local(_) => "local",
            DatabaseBackend::Remote {
                replica_path: Some(_),
                ..
            } => "turso-replica",
            DatabaseBackend::Remote {
                replica_path: None, ..
            } => "turso-remote",
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
