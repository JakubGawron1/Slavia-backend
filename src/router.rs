//! Składanie drzewa tras Axum — rozdzielone od bootstrapu bazy (`create_app` w `lib.rs`).

use axum::{
    Router,
    http::{HeaderName, HeaderValue},
    response::Html,
    routing::{delete, get, patch, post},
};
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::{routes, state::AppState};

async fn backend_root_page() -> Html<&'static str> {
    Html(include_str!("embed/backend_root.html"))
}

pub fn build_router(state: AppState, cors: CorsLayer) -> Router {
    let auth_routes = Router::new()
        .route("/login", post(routes::auth::login_handler))
        .route("/me", get(routes::auth::me_handler))
        .route("/profile", patch(routes::admins::update_profile))
        .route("/totp/setup", post(routes::totp::totp_setup_handler))
        .route("/totp/enable", post(routes::totp::totp_enable_handler))
        .route("/totp/disable", post(routes::totp::totp_disable_handler));

    let upload_routes = Router::new().route("/", post(routes::upload::upload_handler));

    let athletes_routes = Router::new()
        .route(
            "/",
            get(routes::athletes::list_athletes_public).post(routes::athletes::create_athlete),
        )
        .route("/me", get(routes::athletes::me_athlete_handler))
        .route(
            "/my-calendar",
            get(routes::competition_participants::my_calendar_for_athlete),
        )
        .route("/admin", get(routes::athletes::list_athletes))
        .route(
            "/{id}/competitions",
            get(routes::competition_participants::list_competitions_for_athlete)
                .put(routes::competition_participants::sync_competitions_for_athlete),
        )
        .route(
            "/{id}/training-log/{entry_id}",
            patch(routes::training_log::update_training_log)
                .delete(routes::training_log::delete_training_log),
        )
        .route(
            "/{id}/training-log",
            get(routes::training_log::list_training_log)
                .post(routes::training_log::create_training_log),
        )
        .route("/{id}/timeline", get(routes::athletes::athlete_timeline))
        .route("/{id}/link", post(routes::athletes::link_athlete_to_user))
        .route(
            "/{id}/attach-user",
            post(routes::athletes::attach_existing_user_to_athlete),
        )
        .route(
            "/{id}/detach-user",
            post(routes::athletes::detach_user_from_athlete),
        )
        .route(
            "/{id}",
            get(routes::athletes::get_athlete_public)
                .patch(routes::athletes::update_athlete)
                .delete(routes::athletes::delete_athlete),
        );

    let admins_routes = Router::new()
        .route("/grouped", get(routes::admins::list_accounts_grouped))
        .route(
            "/",
            get(routes::admins::list_admins).post(routes::admins::create_admin),
        )
        .route("/{id}", delete(routes::admins::delete_admin))
        .route("/{id}/account", patch(routes::admins::update_user_account))
        .route("/{id}/role", patch(routes::admins::update_user_role))
        .route("/{id}/ban", patch(routes::admins::ban_user))
        .route("/{id}/unban", patch(routes::admins::unban_user))
        .route("/reset", post(routes::admins::reset_database));

    // Zgłoszenia wyników (Pending): osobny `/api/submissions/*`, handlery współdzielone z `results`.
    let submissions_routes = Router::new()
        .route("/pending", get(routes::submissions::list_pending_results))
        .route("/{id}/approve", patch(routes::submissions::approve_result))
        .route("/{id}", delete(routes::submissions::delete_result));

    let results_routes = Router::new()
        .route(
            "/batch-approve",
            post(routes::results::batch_approve_results),
        )
        .route(
            "/public-board",
            get(routes::results::list_public_results_board),
        )
        .route(
            "/public-board-olympic",
            get(routes::results::list_public_olympic_board),
        )
        .route("/all", get(routes::results::list_all_results_staff))
        .route("/pending", get(routes::results::list_pending_results))
        .route(
            "/athlete/{id}/submissions",
            get(routes::results::list_athlete_result_submissions),
        )
        .route("/athlete/{id}", get(routes::results::list_athlete_results))
        .route("/{id}/approve", patch(routes::results::approve_result))
        .route("/{id}/reject", patch(routes::results::reject_result))
        .route(
            "/{id}",
            patch(routes::results::update_result).delete(routes::results::delete_result),
        )
        .route(
            "/",
            get(routes::results::list_approved_results).post(routes::results::create_result),
        );

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
        .route(
            "/manage",
            get(routes::announcements::list_announcements_manage),
        )
        .route(
            "/",
            get(routes::announcements::list_announcements_public)
                .post(routes::announcements::create_announcement),
        )
        .route(
            "/{id}",
            patch(routes::announcements::update_announcement)
                .delete(routes::announcements::delete_announcement),
        );

    let gallery_routes = Router::new()
        .route("/manage", get(routes::gallery::list_gallery_manage))
        .route(
            "/",
            get(routes::gallery::list_gallery_public).post(routes::gallery::create_gallery_photo),
        )
        .route(
            "/{id}",
            patch(routes::gallery::update_gallery_photo)
                .delete(routes::gallery::delete_gallery_photo),
        );

    let contact_routes = Router::new()
        .route(
            "/manage",
            get(routes::contact::list_contact_messages_manage),
        )
        .route("/", post(routes::contact::submit_contact_message))
        .route(
            "/manage/{id}",
            patch(routes::contact::patch_contact_message)
                .delete(routes::contact::delete_contact_message),
        );

    let posts_routes = Router::new()
        .route("/manage", get(routes::posts::list_posts_manage))
        .route("/manage/{id}", get(routes::posts::get_post_manage))
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
        .route(
            "/read-all",
            patch(routes::notifications::mark_all_my_notifications_read),
        )
        .route(
            "/{id}/read",
            patch(routes::notifications::mark_my_notification_read),
        )
        .route(
            "/{id}",
            delete(routes::notifications::delete_my_notification),
        )
        .route(
            "/",
            get(routes::notifications::list_my_notifications)
                .delete(routes::notifications::delete_all_my_notifications),
        );

    let payments_routes = Router::new()
        .route("/my", post(routes::payments::create_my_payment))
        .route("/my/status", get(routes::payments::my_payment_status))
        .route("/my/year", get(routes::payments::my_payments_year))
        .route(
            "/status",
            get(routes::payments::list_athletes_payment_status),
        )
        .route(
            "/overview",
            get(routes::payments::payments_overview_for_month),
        )
        .route("/pending", get(routes::payments::list_pending_payments))
        .route("/{id}/approve", patch(routes::payments::approve_payment))
        .route("/{id}/reject", patch(routes::payments::reject_payment))
        .route(
            "/athlete/{athlete_id}/approved",
            post(routes::payments::create_approved_payment_for_athlete),
        )
        .route(
            "/athlete/{athlete_id}/year",
            get(routes::payments::athlete_payments_year),
        )
        .route(
            "/athlete/{athlete_id}/standing-order",
            patch(routes::payments::set_standing_order),
        );

    let import_routes = Router::new().route("/data", post(routes::import::import_data_handler));
    let exercises_routes = Router::new()
        .route("/board", get(routes::exercises::list_exercises_board))
        .route(
            "/",
            get(routes::exercises::list_exercises).post(routes::exercises::create_exercise),
        )
        .route("/{id}", delete(routes::exercises::delete_exercise));
    let exercise_submissions_routes = Router::new()
        .route(
            "/",
            post(routes::exercise_submissions::create_exercise_submission),
        )
        .route(
            "/my",
            get(routes::exercise_submissions::list_my_exercise_submissions),
        )
        .route(
            "/pending",
            get(routes::exercise_submissions::list_pending_exercise_submissions),
        )
        .route(
            "/{id}/approve",
            patch(routes::exercise_submissions::approve_exercise_submission),
        )
        .route(
            "/{id}/reject",
            patch(routes::exercise_submissions::reject_exercise_submission),
        )
        .route(
            "/board",
            get(routes::exercise_submissions::exercise_board_for_exercise),
        );
    let attendance_routes = Router::new()
        .route(
            "/",
            get(routes::attendance::list_attendance).post(routes::attendance::upsert_attendance),
        )
        .route(
            "/summary/{athlete_id}",
            get(routes::attendance::attendance_summary_for_athlete),
        )
        .route(
            "/record/{id}/verify",
            post(routes::attendance::verify_attendance_record),
        )
        .route(
            "/{athlete_id}",
            get(routes::attendance::list_attendance_for_athlete),
        );
    let chat_routes = Router::new()
        .route(
            "/threads",
            get(routes::chat::list_my_threads).post(routes::chat::open_thread),
        )
        .route(
            "/threads/{thread_id}",
            patch(routes::chat::update_thread).delete(routes::chat::delete_thread),
        )
        .route(
            "/threads/{thread_id}/messages",
            get(routes::chat::list_messages).post(routes::chat::send_message),
        )
        // Ręczne czyszczenie bezczynnych wątków (Admin+). Domyślnie używa progu z `chat_cleanup::CHAT_INACTIVITY_DAYS`,
        // można go nadpisać query paramem `?days=N` (1..=365) — np. by ręcznie posprzątać po incydencie.
        .route("/admin/prune", post(routes::chat::admin_prune_threads));
    let comments_routes = Router::new().route(
        "/",
        get(routes::comments::list_comments).post(routes::comments::create_comment),
    );
    let training_plans_routes = Router::new()
        .route("/my", get(routes::training_plans::list_my_training_plans))
        .route(
            "/athlete/{athlete_id}",
            get(routes::training_plans::list_athlete_training_plans),
        )
        .route("/", post(routes::training_plans::create_training_plan))
        .route(
            "/{id}",
            patch(routes::training_plans::update_training_plan)
                .delete(routes::training_plans::delete_training_plan),
        )
        .route(
            "/{id}/items",
            get(routes::training_plans::list_plan_items)
                .put(routes::training_plans::update_plan_items),
        )
        .route(
            "/{id}/my-progress",
            patch(routes::training_plans::update_my_plan_progress),
        );
    let recovery_routes = Router::new()
        .route(
            "/",
            get(routes::recovery::list_recovery_logs)
                .post(routes::recovery::upsert_my_recovery_log),
        )
        .route(
            "/athlete/{athlete_id}",
            get(routes::recovery::list_recovery_logs_for_athlete),
        );
    let system_routes = Router::new()
        .route("/ping", get(routes::system_logs::ping_backend))
        .route("/audit-logs", get(routes::system_logs::list_audit_logs))
        .route("/metrics", get(routes::system_logs::system_metrics))
        .route("/event-feed", get(routes::system_logs::event_feed))
        .route(
            "/feature-flags",
            get(routes::feature_flags::list_feature_flags),
        )
        .route(
            "/feature-flags/{name}",
            post(routes::feature_flags::upsert_feature_flag),
        );

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
        .nest("/api/payments", payments_routes)
        .nest("/api/import", import_routes)
        .nest("/api/exercises", exercises_routes)
        .nest("/api/exercise-submissions", exercise_submissions_routes)
        .nest("/api/attendance", attendance_routes)
        .nest("/api/chat", chat_routes)
        .nest("/api/comments", comments_routes)
        .nest("/api/training-plans", training_plans_routes)
        .nest("/api/recovery", recovery_routes)
        .nest("/api/system", system_routes)
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-api-version"),
            HeaderValue::from_static("1"),
        ))
        .layer(cors)
        .with_state(state)
}
