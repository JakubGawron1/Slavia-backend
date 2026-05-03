//! Składanie drzewa tras Axum — rozdzielone od bootstrapu bazy (`create_app` w `lib.rs`).

use axum::{
    response::Html,
    routing::{delete, get, patch, post},
    Router,
};
use tower_http::cors::CorsLayer;

use crate::{routes, state::AppState};

async fn backend_root_page() -> Html<&'static str> {
    Html(include_str!("embed/backend_root.html"))
}

pub fn build_router(state: AppState, cors: CorsLayer) -> Router {
    let auth_routes = Router::new()
        .route("/login", post(routes::auth::login_handler))
        .route("/me", get(routes::auth::me_handler))
        .route("/profile", patch(routes::admins::update_profile));

    let upload_routes = Router::new()
        .route("/", post(routes::upload::upload_handler));

    let athletes_routes = Router::new()
        .route("/", get(routes::athletes::list_athletes_public).post(routes::athletes::create_athlete))
        .route("/me", get(routes::athletes::me_athlete_handler))
        .route(
            "/my-calendar",
            get(routes::competition_participants::my_calendar_for_athlete),
        )
        .route("/admin", get(routes::athletes::list_athletes))
        .route(
            "/{id}/competitions",
            get(routes::competition_participants::list_competitions_for_athlete).put(
                routes::competition_participants::sync_competitions_for_athlete,
            ),
        )
        .route(
            "/{id}/training-log/{entry_id}",
            patch(routes::training_log::update_training_log)
                .delete(routes::training_log::delete_training_log),
        )
        .route(
            "/{id}/training-log",
            get(routes::training_log::list_training_log).post(routes::training_log::create_training_log),
        )
        .route("/{id}/link", post(routes::athletes::link_athlete_to_user))
        .route(
            "/{id}",
            patch(routes::athletes::update_athlete).delete(routes::athletes::delete_athlete),
        );

    let admins_routes = Router::new()
        .route("/grouped", get(routes::admins::list_accounts_grouped))
        .route("/", get(routes::admins::list_admins).post(routes::admins::create_admin))
        .route("/{id}", delete(routes::admins::delete_admin))
        .route("/{id}/account", patch(routes::admins::update_user_account))
        .route("/{id}/role", patch(routes::admins::update_user_role))
        .route("/reset", post(routes::admins::reset_database));

    // Zgłoszenia wyników (Pending): osobny `/api/submissions/*`, handlery współdzielone z `results`.
    let submissions_routes = Router::new()
        .route("/pending", get(routes::submissions::list_pending_results))
        .route("/{id}/approve", patch(routes::submissions::approve_result))
        .route("/{id}", delete(routes::submissions::delete_result));

    let results_routes = Router::new()
        .route("/public-board", get(routes::results::list_public_results_board))
        .route("/all", get(routes::results::list_all_results_staff))
        .route("/pending", get(routes::results::list_pending_results))
        .route(
            "/athlete/{id}/submissions",
            get(routes::results::list_athlete_result_submissions),
        )
        .route("/athlete/{id}", get(routes::results::list_athlete_results))
        .route("/{id}/approve", patch(routes::results::approve_result))
        .route(
            "/{id}",
            patch(routes::results::update_result).delete(routes::results::delete_result),
        )
        .route("/", get(routes::results::list_approved_results).post(routes::results::create_result));

    let competitions_routes = Router::new()
        .route(
            "/sync-external",
            post(routes::competitions::sync_external_competitions),
        )
        .route(
            "/recurring-training-cancellations/{session_date}",
            delete(routes::recurring_training_cancellations::restore_recurring_training_session),
        )
        .route(
            "/recurring-training-cancellations",
            get(routes::recurring_training_cancellations::list_recurring_training_cancellations)
                .post(routes::recurring_training_cancellations::upsert_recurring_training_session)
                .delete(routes::recurring_training_cancellations::clear_all_recurring_training_cancellations),
        )
        .route(
            "/",
            get(routes::competitions::list_competitions)
                .post(routes::competitions::create_competition),
        )
        .route(
            "/{id}/participants",
            get(routes::competition_participants::list_participants).put(
                routes::competition_participants::set_participants,
            ),
        )
        .route("/{id}", delete(routes::competitions::delete_competition).patch(routes::competitions::update_competition));

    let announcements_routes = Router::new()
        .route("/manage", get(routes::announcements::list_announcements_manage))
        .route(
            "/",
            get(routes::announcements::list_announcements_public).post(routes::announcements::create_announcement),
        )
        .route(
            "/{id}",
            patch(routes::announcements::update_announcement).delete(routes::announcements::delete_announcement),
        );

    let gallery_routes = Router::new()
        .route("/manage", get(routes::gallery::list_gallery_manage))
        .route(
            "/",
            get(routes::gallery::list_gallery_public).post(routes::gallery::create_gallery_photo),
        )
        .route(
            "/{id}",
            patch(routes::gallery::update_gallery_photo).delete(routes::gallery::delete_gallery_photo),
        );

    let contact_routes = Router::new()
        .route("/manage", get(routes::contact::list_contact_messages_manage))
        .route("/", post(routes::contact::submit_contact_message))
        .route(
            "/manage/{id}",
            patch(routes::contact::patch_contact_message).delete(routes::contact::delete_contact_message),
        );

    let posts_routes = Router::new()
        .route(
            "/manage",
            get(routes::posts::list_posts_manage),
        )
        .route(
            "/manage/{id}",
            get(routes::posts::get_post_manage),
        )
        .route(
            "/",
            get(routes::posts::list_posts_public).post(routes::posts::create_post),
        )
        .route(
            "/{id}",
            get(routes::posts::get_post_public)
                .patch(routes::posts::update_post)
                .delete(routes::posts::delete_post),
        );

    let notifications_routes = Router::new()
        .route("/{id}", delete(routes::notifications::delete_my_notification))
        .route("/", get(routes::notifications::list_my_notifications));

    Router::new()
        .route("/", get(backend_root_page))
        .nest("/api/auth", auth_routes)
        .nest("/api/upload", upload_routes)
        .nest("/api/athletes", athletes_routes)
        .nest("/api/admins", admins_routes)
        .nest("/api/submissions", submissions_routes)
        .nest("/api/results", results_routes)
        .nest("/api/competitions", competitions_routes)
        .nest("/api/posts", posts_routes)
        .nest("/api/announcements", announcements_routes)
        .nest("/api/gallery", gallery_routes)
        .nest("/api/contact", contact_routes)
        .nest("/api/notifications", notifications_routes)
        .layer(cors)
        .with_state(state)
}
