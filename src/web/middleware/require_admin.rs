use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};

use crate::web::session::SessionData;

/// Route-group auth: gate everything below this layer on admin. `SessionData`'s
/// extractor defaults to `Anonymous` when there's no session, so an
/// unauthenticated request gets a clean `403`, not a panic.
///
/// Wired via `axum::middleware::from_fn(require_admin)` on an `admin` router nest.
pub async fn require_admin(
    session_data: SessionData,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    if session_data.auth_state.is_admin() {
        Ok(next.run(req).await)
    } else {
        Err((StatusCode::FORBIDDEN, "Admin only"))
    }
}
