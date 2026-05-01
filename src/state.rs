use std::sync::Arc;
use libsql::Connection;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Connection>,
    pub jwt_secret: String,
    pub cloudinary_cloud_name: String,
    pub cloudinary_api_key: String,
    pub cloudinary_api_secret: String,
}
