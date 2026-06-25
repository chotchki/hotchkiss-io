use axum::{middleware::from_fn, routing::get, Router};

use crate::web::{app_state::AppState, middleware::require_admin::require_admin};

pub mod analytics;
pub mod pages;

/// Everything under `/admin` — gated as a group by the `require_admin` layer,
/// so handlers inside don't repeat the check.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/analytics", get(analytics::show_analytics))
        .route("/pages", get(pages::show_admin_pages))
        .layer(from_fn(require_admin))
}
