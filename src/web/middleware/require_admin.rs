use axum::{extract::Request, middleware::Next, response::Response};

use crate::web::error_page::{forbidden_response, unauthorized_response};
use crate::web::session::SessionData;

/// Route-group auth: gate everything below this layer on admin. `SessionData`'s
/// extractor defaults to `Anonymous` when there's no session. A MISSING identity
/// gets a 401; an authenticated-but-insufficient caller gets a 403 (DK.2) — never
/// a panic.
///
/// Wired via `axum::middleware::from_fn(require_admin)` on an `admin` router nest.
pub async fn require_admin(session_data: SessionData, req: Request, next: Next) -> Response {
    if session_data.auth_state.is_admin() {
        next.run(req).await
    } else if session_data.auth_state.is_authenticated() {
        forbidden_response(req.headers())
    } else {
        unauthorized_response(req.headers())
    }
}
