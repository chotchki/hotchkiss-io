//! Greylist enforcement (CX.5): toll a greylisted IP unless it's exempt, cleared, or an
//! authenticated user. Reads the in-memory snapshot (no DB on the hot path). Layered INNER to
//! `refresh_session_role` so `SessionData` reflects the live role / API-key identity.

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap},
    middleware::Next,
    response::Response,
};
use sqlx::types::chrono::Utc;

use crate::greylist::challenge::verify_clearance;
use crate::web::app_state::AppState;
use crate::web::features::challenge::{render_interstitial, CLEARANCE_COOKIE};
use crate::web::session::SessionData;

/// Marker inserted on a tolled `429` response so the (outer) request-log middleware stamps
/// `challenged` + forces `is_bot` — a challenged request is provably not a human.
#[derive(Clone, Copy)]
pub struct Challenged;

/// Prefixes exempt from the toll even for a greylisted IP: the toll itself + its endpoints (MUST
/// stay reachable or a greylisted client could never solve it), the interstitial's own static
/// assets (or the page can't render), and the operational well-knowns. `/media` is deliberately
/// NOT here — the big content stays behind the toll.
const EXEMPT_PREFIXES: &[&str] = &[
    "/challenge",
    "/login", // a greylisted human can still authenticate (then is_authenticated waves them through)
    "/styles/",
    "/scripts/",
    "/images/",
    "/vendor/",
    "/.well-known/",
    "/robots.txt",
    "/sitemap.xml",
];

fn is_exempt_path(path: &str) -> bool {
    path == "/favicon.ico" || EXEMPT_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// Pull one cookie value from the `Cookie` header (no cookie crate needed for a single lookup).
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        part.trim()
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
            .map(|v| v.to_string())
    })
}

pub async fn greylist_challenge(
    State(state): State<AppState>,
    session: SessionData,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let ip = peer.ip().to_string();

    // Not greylisted → straight through (one short read lock, no DB).
    if !state.greylist.is_greylisted(&ip) {
        return next.run(req).await;
    }

    // Exempt paths (the toll + its assets + operational well-knowns) always pass.
    let path = req.uri().path().to_string();
    if is_exempt_path(&path) {
        return next.run(req).await;
    }

    // An authenticated human (session cookie or API key) is never tolled.
    if session.auth_state.is_authenticated() {
        return next.run(req).await;
    }

    // A valid clearance cookie passes.
    let now = Utc::now().timestamp();
    let cleared = cookie_value(req.headers(), CLEARANCE_COOKIE)
        .map(|token| verify_clearance(&state.challenge.key, &token, now))
        .unwrap_or(false);
    if cleared {
        return next.run(req).await;
    }

    // Toll them: serve the interstitial; redirect target = the URL they were trying to reach.
    let redir = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let mut resp = render_interstitial(&redir);
    resp.extensions_mut().insert(Challenged);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exempt_paths_cover_the_toll_and_its_assets_not_media() {
        assert!(is_exempt_path("/challenge"));
        assert!(is_exempt_path("/challenge/new"));
        assert!(is_exempt_path("/scripts/challenge.js"));
        assert!(is_exempt_path("/styles/main.css"));
        assert!(is_exempt_path("/images/404/blame_bonnie.avif"));
        assert!(is_exempt_path("/favicon.ico"));
        assert!(is_exempt_path("/robots.txt"));
        assert!(is_exempt_path("/login/finish_authentication"), "can still log in while greylisted");
        assert!(!is_exempt_path("/"), "the site content is tolled");
        assert!(!is_exempt_path("/pages/projects"));
        assert!(
            !is_exempt_path("/media/file/abc"),
            "big content stays behind the toll"
        );
    }

    #[test]
    fn cookie_value_extracts_the_named_cookie() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            "id=abc; hio_toll=TOKEN123; other=x".parse().unwrap(),
        );
        assert_eq!(cookie_value(&h, "hio_toll").as_deref(), Some("TOKEN123"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }
}
