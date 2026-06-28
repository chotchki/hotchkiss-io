use axum::{extract::Request, http::Method, middleware::Next, response::Response};

use crate::web::error_page::forbidden_response;
use crate::web::session::SessionData;

/// Fail-closed, site-wide authorization (Phase E). The site's default is
/// **read is public, writing requires admin** — so a newly-added mutating route
/// is protected automatically instead of relying on someone remembering a check
/// (the failure mode that caused the v0.0.49 anonymous-mutation bug).
///
/// - `GET` / `HEAD` / `OPTIONS` are public everywhere (safe, side-effect-free).
/// - EVERY other method requires an authenticated Admin BY DEFAULT. This is a
///   default-DENY allowlist of safe methods, NOT a deny-list of known-mutating
///   verbs — so an exotic method (e.g. the `PATCH` on `/pages/preview`) is gated
///   without anyone enumerating it.
/// - The ONLY exceptions are the WebAuthn login-ceremony POSTs (the caller isn't
///   authenticated yet) + the debug-only test-login seam. Matching is EXACT
///   `(path, method)` — never a prefix — so it can't silently widen to a future
///   `/login/*` sibling.
///
/// `SessionData` defaults to `Anonymous` when there's no session, so an
/// unauthenticated write gets a clean `403`, not a panic. Wired INNER to the
/// session layer (so the session is loaded) in `router.rs`.
///
/// NOTE: the `/admin` nest keeps its own `require_admin` layer — it gates a
/// *non-public GET* (`/admin/analytics`) that this GET-public default would
/// otherwise expose. The two layers are complementary, not redundant.
pub async fn require_admin_for_mutations(
    session_data: SessionData,
    req: Request,
    next: Next,
) -> Response {
    // Reads are public site-wide.
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }

    // The only non-GET endpoints a non-admin may reach: the anonymous auth
    // ceremony (exact path + method, never a prefix).
    if is_anonymous_auth_endpoint(req.method(), req.uri().path()) {
        return next.run(req).await;
    }

    if session_data.auth_state.is_admin() {
        next.run(req).await
    } else {
        forbidden_response(req.headers())
    }
}

/// EXACT `(path, method)` allowlist of the only non-GET endpoints reachable
/// without admin: the two WebAuthn ceremony POSTs (registration / authentication
/// *finish*), plus the debug-only `/test/login` seam (`cfg`-gated to match the
/// route, which is absent from release). The ceremony's GET steps
/// (`/login`, `/login/get_auth_opts`, `/login/start_register/{name}`,
/// `/login/logout`) are covered by the GET-public rule, so they need no entry.
/// CAVEAT: if logout or start_register is ever changed to a POST it MUST be added
/// here or non-admins can't log in/out.
fn is_anonymous_auth_endpoint(method: &Method, path: &str) -> bool {
    if method != Method::POST {
        return false;
    }
    match path {
        "/login/finish_authentication" | "/login/finish_register" => true,
        #[cfg(debug_assertions)]
        "/test/login" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_is_post_only_and_exact() {
        // The two real ceremony POSTs are allowlisted.
        assert!(is_anonymous_auth_endpoint(
            &Method::POST,
            "/login/finish_authentication"
        ));
        assert!(is_anonymous_auth_endpoint(
            &Method::POST,
            "/login/finish_register"
        ));

        // POST-only: a different verb to the same path is NOT exempt.
        assert!(!is_anonymous_auth_endpoint(
            &Method::PUT,
            "/login/finish_register"
        ));

        // EXACT, never a prefix: siblings + parents are NOT exempt.
        assert!(!is_anonymous_auth_endpoint(&Method::POST, "/login"));
        assert!(!is_anonymous_auth_endpoint(
            &Method::POST,
            "/login/finish_register/evil"
        ));
        assert!(!is_anonymous_auth_endpoint(&Method::POST, "/pages/"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_login_seam_allowlisted_in_debug() {
        assert!(is_anonymous_auth_endpoint(&Method::POST, "/test/login"));
    }
}
