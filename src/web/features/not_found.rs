use askama::Template;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use http::StatusCode;
use sqlx::SqlitePool;
use tracing::error;

use crate::web::{
    app_state::AppState, authentication_state::AuthenticationState, features::top_bar::TopBar,
    html_template::HtmlTemplate, session::SessionData,
};

#[derive(Template)]
#[template(path = "404.html")]
pub struct NotFoundTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
}

/// Render the "blame the cat" 404 with a real `404` status. Shared by the
/// router fallback (unmatched routes) AND the `/pages/*` miss branch, so there
/// is exactly ONE 404 page. `HtmlTemplate` always renders `200`, so we wrap it
/// in `(NOT_FOUND, _)` to override the status while keeping the HTML body.
///
/// Building the nav needs the DB; if that query fails we STILL return a 404
/// (a plain one), never a 500 — a flaky nav must not turn "page not found"
/// into "server error".
pub async fn render_not_found(pool: &SqlitePool, auth_state: AuthenticationState) -> Response {
    match TopBar::create(pool, "", auth_state.role()).await {
        Ok(top_bar) => (
            StatusCode::NOT_FOUND,
            HtmlTemplate(NotFoundTemplate {
                top_bar,
                auth_state,
            }),
        )
            .into_response(),
        Err(e) => {
            error!("404 page nav build failed: {e:?}");
            (StatusCode::NOT_FOUND, "404 - Not Found").into_response()
        }
    }
}

/// Axum router fallback: a request that matched no route. The authz layer keeps
/// GET/HEAD/OPTIONS public, so safe-method misses (e.g. `/nope`) land here;
/// other methods are already 403'd before routing. Attached BEFORE `with_state`
/// so it can extract `State<AppState>`.
pub async fn fallback(State(state): State<AppState>, session_data: SessionData) -> Response {
    render_not_found(&state.pool, session_data.auth_state).await
}
