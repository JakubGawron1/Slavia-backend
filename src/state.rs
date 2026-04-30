use std::sync::Arc;
use libsql::Connection;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Connection>,
    pub jwt_secret: String,
}
