//! Greylist enforcement (CX.5): toll a greylisted IP unless it's exempt, cleared, or an
//! authenticated user. Reads the in-memory snapshot (no DB on the hot path). Layered INNER to
//! `refresh_session_role` so `SessionData` reflects the live role / API-key identity.
//!
//! A non-browser (JSON) client — an MCP client, an API caller — gets a machine-readable greylist
//! notice instead of the JS proof-of-work interstitial it has no engine to solve (Phase DI).

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sqlx::types::chrono::Utc;

use crate::greylist::challenge::verify_clearance;
use crate::web::app_state::AppState;
use crate::web::features::challenge::{render_interstitial, CLEARANCE_COOKIE};
use crate::web::responder::ClientKind;
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
    // `/library` is EXACT, not a prefix (Phase DE): a greylisted logged-out
    // family member must reach the sign-in gate, and the gate page serves
    // nothing worth scraping — but the SUBTREE (book listings under
    // /library/*, /pages/library/*) stays behind the toll like any content.
    path == "/favicon.ico"
        || path == "/library"
        || EXEMPT_PREFIXES.iter().any(|p| path.starts_with(p))
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

    // Toll them. The interstitial is a JS proof-of-work page — a NON-BROWSER client (an MCP
    // client, an API caller: anything sending `Accept: application/json`) has no JS engine and
    // can't solve it, so serving the HTML is useless. Give it a clear, machine-readable notice
    // instead; a browser still gets the real interstitial (redirect target = the URL it was
    // trying to reach). Either way the `Challenged` marker forces the log to stamp challenged +
    // is_bot.
    let redir = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let mut resp = if ClientKind::from_headers(req.headers()) == ClientKind::Json {
        greylist_json_notice()
    } else {
        render_interstitial(&redir)
    };
    resp.extensions_mut().insert(Challenged);
    resp
}

/// The machine-readable toll for a non-browser (JSON) client — it can't solve the PoW, so tell it
/// plainly why it's blocked and how a human behind it can proceed (authenticate, or use a browser).
fn greylist_json_notice() -> Response {
    let body = serde_json::json!({
        "error": "greylisted",
        "message": "This IP is greylisted for abusive traffic. The proof-of-work challenge to clear \
                    the toll requires a JavaScript-capable browser — an automated client can't pass \
                    it. Authenticate with an API key (Authorization: Bearer hio_…) to bypass the \
                    toll, or open the site in a browser and complete the challenge.",
    });
    (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response()
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
    fn json_notice_is_a_429_json_body_naming_the_greylist() {
        let r = greylist_json_notice();
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            r.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
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
