use std::sync::Arc;

use libsql::Connection;

use crate::DatabaseBackend;

fn is_stream_not_found_error(e: &libsql::Error) -> bool {
    let msg = e.to_string().to_ascii_lowercase();
    // Turso/libsql (Hrana) potrafi zwrócić 404 ze starym streamem po idle/redeployu.
    msg.contains("stream not found")
}

#[derive(Clone)]
pub struct Db {
    backend: DatabaseBackend,
    conn: Arc<tokio::sync::RwLock<Arc<Connection>>>,
}

impl Db {
    pub async fn new(backend: DatabaseBackend) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let conn = Arc::new(connect_database(backend.clone()).await?);
        Ok(Self {
            backend,
            conn: Arc::new(tokio::sync::RwLock::new(conn)),
        })
    }

    async fn reconnect(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let next = Arc::new(connect_database(self.backend.clone()).await?);
        let mut w = self.conn.write().await;
        *w = next;
        Ok(())
    }

    /// Dostęp do “surowego” połączenia (np. do migracji/initu).
    pub async fn raw(&self) -> Arc<Connection> {
        self.conn.read().await.clone()
    }

    pub async fn execute<P>(&self, sql: &str, params: P) -> Result<u64, libsql::Error>
    where
        P: libsql::params::IntoParams + Clone + Send,
    {
        let conn = self.conn.read().await.clone();
        match conn.execute(sql, params.clone()).await {
            Ok(v) => Ok(v),
            Err(e) if is_stream_not_found_error(&e) => {
                // Jednorazowy reconnect + retry.
                let _ = self.reconnect().await;
                let conn2 = self.conn.read().await.clone();
                conn2.execute(sql, params).await
            }
            Err(e) => Err(e),
        }
    }

    pub async fn query<P>(&self, sql: &str, params: P) -> Result<libsql::Rows, libsql::Error>
    where
        P: libsql::params::IntoParams + Clone + Send,
    {
        let conn = self.conn.read().await.clone();
        match conn.query(sql, params.clone()).await {
            Ok(v) => Ok(v),
            Err(e) if is_stream_not_found_error(&e) => {
                let _ = self.reconnect().await;
                let conn2 = self.conn.read().await.clone();
                conn2.query(sql, params).await
            }
            Err(e) => Err(e),
        }
    }
}

async fn connect_database(
    backend: DatabaseBackend,
) -> Result<Connection, Box<dyn std::error::Error + Send + Sync>> {
    match backend {
        DatabaseBackend::Local(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let db = libsql::Builder::new_local(path).build().await?;
            Ok(db.connect()?)
        }
        DatabaseBackend::Remote { url, auth_token } => {
            let db = libsql::Builder::new_remote(url, auth_token).build().await?;
            Ok(db.connect()?)
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
}
