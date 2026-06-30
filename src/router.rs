use axum::Router;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

use crate::AppState;
use crate::admin;
use crate::scripts;
use crate::webhook;

pub fn build_router(state: AppState) -> Router {
    let public = Router::new()
        .route(
            "/{repo_uuid}/{script_uuid}/{slug}",
            get(scripts::serve_userscript),
        )
        .route("/webhook/{repo_uuid}", post(webhook::handle_webhook))
        .route("/health", get(|| async { r#"{"ok":true}"# }));

    let admin_routes = Router::new()
        .route("/", get(admin::handlers::list))
        .route(
            "/scripts/{repo_uuid}/{script_uuid}/edit",
            get(admin::handlers::edit_form),
        )
        .route(
            "/scripts/{repo_uuid}/{script_uuid}/override",
            post(admin::handlers::edit_post),
        )
        .route(
            "/scripts/{repo_uuid}/{script_uuid}/uuid",
            get(admin::handlers::script_uuid_form).post(admin::handlers::script_uuid_post),
        )
        .route(
            "/scripts/{repo_uuid}/{script_uuid}/toggle-disabled",
            post(admin::handlers::toggle_disabled_post),
        )
        .route(
            "/repos/{repo_uuid}/uuid",
            get(admin::handlers::repo_uuid_form).post(admin::handlers::repo_uuid_post),
        )
        .route("/repos/{repo_uuid}/pull", post(admin::handlers::pull_post))
        .layer(admin::auth::basic_auth_layer(&state.config));

    Router::new()
        .merge(public)
        .nest("/admin", admin_routes)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
