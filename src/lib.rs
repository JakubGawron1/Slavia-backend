//! Współdzielona logika HTTP — używana przez `main` (Axum/Tokio) i testy.

use axum::{
    routing::{delete, get, patch, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

pub mod db;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod state;

use state::AppState;

/// Buduje router Axum (Turso/libsql + JWT). Bez `Box::pin` — mniejsze ryzyko problemów ze stosem na Windows.
pub async fn create_app(
    db_url: &str,
    db_token: &str,
    jwt_secret: String,
    cloudinary_cloud_name: String,
    cloudinary_api_key: String,
    cloudinary_api_secret: String,
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    let client = libsql::Builder::new_remote(db_url.to_string(), db_token.to_string())
        .build()
        .await?;

    let conn = client.connect()?;

    db::init_db(&conn).await?;

    let state = AppState {
        db: Arc::new(conn),
        jwt_secret,
        cloudinary_cloud_name,
        cloudinary_api_key,
        cloudinary_api_secret,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let auth_routes = Router::new()
        .route("/login", post(routes::auth::login_handler))
        .route("/me", get(routes::auth::me_handler));

    let upload_routes = Router::new()
        .route("/", post(routes::upload::upload_handler));

    let athletes_routes = Router::new()
        .route("/", get(routes::athletes::list_athletes_public).post(routes::athletes::create_athlete))
        .route("/me", get(routes::athletes::me_athlete_handler))
        .route("/admin", get(routes::athletes::list_athletes))
        .route("/{id}", patch(routes::athletes::update_athlete).delete(routes::athletes::delete_athlete))
        .route("/{id}/link", post(routes::athletes::link_athlete_to_user));

    let admins_routes = Router::new()
        .route("/", get(routes::admins::list_admins).post(routes::admins::create_admin))
        .route("/{id}", delete(routes::admins::delete_admin));

    let results_routes = Router::new()
        .route("/", get(routes::results::list_approved_results).post(routes::results::create_result))
        .route("/athlete/{id}", get(routes::results::list_athlete_results))
        .route("/pending", get(routes::results::list_pending_results))
        .route("/{id}/approve", patch(routes::results::approve_result));

    let competitions_routes = Router::new()
        .route(
            "/",
            get(routes::competitions::list_competitions)
                .post(routes::competitions::create_competition),
        )
        .route("/{id}", delete(routes::competitions::delete_competition).patch(routes::competitions::update_competition));

    let posts_routes = Router::new()
        .route(
            "/",
            get(routes::posts::list_posts).post(routes::posts::create_post),
        )
        .route("/{id}", get(routes::posts::get_post).delete(routes::posts::delete_post));

    let app = Router::new()
        .route(
            "/",
            get(|| async {
                axum::response::Html(
                    "<!DOCTYPE html>
                <html lang=\"pl\">
                <head>
                    <meta charset=\"UTF-8\">
                    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">
                    <title>Slavia Backend</title>
                    <style>
                        body { font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif; background-color: #121212; color: #ffffff; display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100vh; margin: 0; }
                        h1 { color: #e50914; }
                        p { font-size: 1.2rem; color: #cccccc; }
                    </style>
                </head>
                <body>
                    <h1>CKS Slavia Ruda Śląska</h1>
                    <p>Witaj! To jest oficjalny serwer backendowy (API) klubu podnoszenia ciężarów.</p>
                </body>
                </html>",
                )
            }),
        )
        .nest("/api/auth", auth_routes)
        .nest("/api/upload", upload_routes)
        .nest("/api/athletes", athletes_routes)
        .nest("/api/admins", admins_routes)
        .nest("/api/results", results_routes)
        .nest("/api/competitions", competitions_routes)
        .nest("/api/posts", posts_routes)
        .layer(cors)
        .with_state(state);

    Ok(app)
}
