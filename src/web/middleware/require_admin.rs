use axum::{extract::Request, middleware::Next, response::Response};

use crate::web::error_page::forbidden_response;
use crate::web::session::SessionData;

/// Route-group auth: gate everything below this layer on admin. `SessionData`'s
/// extractor defaults to `Anonymous` when there's no session, so an
/// unauthenticated request gets the styled `403`, not a panic.
///
/// Wired via `axum::middleware::from_fn(require_admin)` on an `admin` router nest.
pub async fn require_admin(session_data: SessionData, req: Request, next: Next) -> Response {
    if session_data.auth_state.is_admin() {
        next.run(req).await
    } else {
        forbidden_response(req.headers())
    }
}
