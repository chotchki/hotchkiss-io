use axum::{
    extract::DefaultBodyLimit,
    middleware::from_fn,
    routing::{delete, get, post},
    Router,
};

use crate::web::{app_state::AppState, middleware::require_admin::require_admin};

pub mod analytics;
pub mod api_keys;
pub mod media;
pub mod pages;

/// Everything under `/admin` — gated as a group by the `require_admin` layer,
/// so handlers inside don't repeat the check.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/analytics", get(analytics::show_analytics))
        .route("/pages", get(pages::show_admin_pages))
        .route("/pages/reorder", post(pages::reorder_pages))
        // Media library (Phase BZ). Upload disables the body limit for video.
        .route("/media", get(media::show_media_library))
        .route(
            "/media/upload",
            post(media::upload_media).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/media/{media_id}/encode",
            post(media::add_encode).layer(DefaultBodyLimit::disable()),
        )
        .route("/media/{media_id}/rename", post(media::rename_media))
        .route("/media/variant/{variant_id}", delete(media::delete_variant))
        .route("/media/{media_id}", delete(media::delete_media))
        // API keys (Phase CA): generate (shown once) / list / revoke your own.
        .route(
            "/api-keys",
            get(api_keys::show_api_keys).post(api_keys::create_api_key),
        )
        .route("/api-keys/{id}", delete(api_keys::revoke_api_key))
        .layer(from_fn(require_admin))
}
