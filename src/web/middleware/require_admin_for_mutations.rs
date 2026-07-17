use axum::{extract::Request, http::Method, middleware::Next, response::Response};
use tracing::{info, warn};

use crate::db::dao::roles::Role;
use crate::web::error_page::{forbidden_response, unauthorized_response};
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
///   authenticated yet) + the debug-only test-login seam, and the role-scoped
///   allowlist below (Phase CZ — currently EMPTY). Matching is EXACT
///   `(path, method)` — never a prefix — so it can't silently widen to a future
///   `/login/*` sibling.
///
/// Decision order: safe methods pass → anonymous WebAuthn allowlist →
/// role-scoped allowlist (rank-checked) → admin fallback → 403.
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

    // Role-scoped entries: a mutation a sub-admin tier may perform, gated by
    // rank. Checked BEFORE the admin fallback (an admin outranks every entry
    // anyway, so order only matters for non-admins).
    if allowed_by_role_scope(
        ROLE_SCOPED_MUTATIONS,
        req.method(),
        req.uri().path(),
        session_data.auth_state.role(),
    ) {
        return next.run(req).await;
    }

    // Missing identity → 401 (who are you?); authenticated but insufficient → 403
    // (DK.2). No `WWW-Authenticate` on the 401 — it would trigger an MCP client's
    // OAuth discovery and a browser basic-auth prompt.
    if session_data.auth_state.is_admin() {
        next.run(req).await
    } else if session_data.auth_state.is_authenticated() {
        // A logged-in NON-admin denied a mutation is genuinely notable (a real
        // account, or a stale/compromised session, hitting an admin route) —
        // WARN so the /admin/logs warn|error filter surfaces it (DM.9).
        warn!(
            method = %req.method(),
            path = req.uri().path(),
            role = %session_data.auth_state.role(),
            "mutation denied: authenticated but not admin (403)"
        );
        forbidden_response(req.headers())
    } else {
        // Anonymous mutation attempts are mostly bot probes — INFO (visible in
        // the "all" view, kept OUT of warn|error) so the routine noise doesn't
        // drown the notable 403s above.
        info!(
            method = %req.method(),
            path = req.uri().path(),
            "mutation denied: no valid identity (401)"
        );
        unauthorized_response(req.headers())
    }
}

/// Role-scoped mutation allowlist (Phase CZ — the seam the Library progress
/// endpoint and the future Home tab consume): exact-match `(method, path)` →
/// the MINIMUM `Role` whose `rank()` may pass. Two conventions keep this a
/// table and not a router: **resource ids ride the request BODY** (so entries
/// stay exact-match, never a path pattern), and **per-resource authorization
/// beyond the coarse role gate lives in the handler** (e.g. a progress save
/// re-checks the media's own `min_role`). Entries are code, reviewed like the
/// WebAuthn ones above. Shipped EMPTY — Phase DF adds the first entry
/// (`POST /library/progress` at `Role::Family`).
const ROLE_SCOPED_MUTATIONS: &[(Method, &str, Role)] = &[];

/// True when `(method, path)` has a role-scoped entry AND the viewer's rank
/// meets it. Exact match only — a prefix or sibling never qualifies.
/// Parameterized on the table so tests exercise the matching with a fixture
/// while the shipped table stays empty.
fn allowed_by_role_scope(
    table: &[(Method, &str, Role)],
    method: &Method,
    path: &str,
    viewer: Role,
) -> bool {
    table
        .iter()
        .find(|(m, p, _)| m == method && *p == path)
        .is_some_and(|(_, _, min)| viewer.rank() >= min.rank())
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

    /// The shipped table is EMPTY by design in Phase CZ — the machinery lands
    /// inert; Phase DF adds the first entry. If an entry appears here, move it
    /// into the fixture coverage below too.
    #[test]
    fn shipped_role_scope_table_is_empty() {
        assert!(ROLE_SCOPED_MUTATIONS.is_empty());
    }

    #[test]
    fn role_scope_matching_is_exact_and_rank_gated() {
        const FIXTURE: &[(Method, &str, Role)] =
            &[(Method::POST, "/library/progress", Role::Family)];

        // Rank gate: Family meets a Family-min entry, Admin outranks it,
        // Registered and Anonymous fall short.
        for (viewer, allowed) in [
            (Role::Anonymous, false),
            (Role::Registered, false),
            (Role::Family, true),
            (Role::Admin, true),
        ] {
            assert_eq!(
                allowed_by_role_scope(FIXTURE, &Method::POST, "/library/progress", viewer),
                allowed,
                "viewer {viewer}"
            );
        }

        // Exact method: same path, different verb → no entry, no pass.
        assert!(!allowed_by_role_scope(
            FIXTURE,
            &Method::PUT,
            "/library/progress",
            Role::Admin
        ));

        // Exact path: children, parents and siblings never inherit the entry.
        for path in ["/library/progress/123", "/library", "/library/progressx"] {
            assert!(
                !allowed_by_role_scope(FIXTURE, &Method::POST, path, Role::Admin),
                "path {path} must not match"
            );
        }

        // An empty table (the shipped state) allows nothing.
        assert!(!allowed_by_role_scope(
            &[],
            &Method::POST,
            "/library/progress",
            Role::Admin
        ));
    }
}
