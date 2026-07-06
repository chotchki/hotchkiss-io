//! The greylist bot-challenge interstitial + its endpoints (CX.6).
//!
//! Flow: the enforcement middleware (CX.5) serves [`render_interstitial`] (a STATIC 429 page)
//! to a greylisted request. Its JS calls `GET /challenge/new` for a fresh signed token, fetches
//! the raw-RGBA toll image from `GET /challenge/image/{version}`, runs the image chain + HMAC in
//! a Web Worker, then hits `GET /challenge/verify` — which recomputes and constant-time-compares
//! the answer, and on success mints a bearer clearance cookie and 302s back to the original URL.
//!
//! All routes are public GETs (the fail-closed authz layer allows GET). `/challenge/*` MUST be
//! exempted by the enforcement middleware, or a greylisted client could never reach the toll.

use std::net::SocketAddr;

use askama::Template;
use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{header, HeaderName, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::Utc;

use crate::db::dao::greylist::GreylistDao;
use crate::greylist::challenge::{
    derive_seed, mint_clearance, verify_answer, ChallengeParams, FRESHNESS_WINDOW,
};
use crate::web::app_error::AppError;
use crate::web::app_state::AppState;

/// The clearance cookie name + lifetime. Bearer token, NOT IP-bound (design doc).
pub const CLEARANCE_COOKIE: &str = "hio_toll";
pub const CLEARANCE_TTL_DAYS: i64 = 7;

pub fn challenge_router() -> Router<AppState> {
    Router::new()
        .route("/", get(show_interstitial))
        .route("/new", get(new_challenge))
        .route("/verify", get(verify_challenge))
        .route("/image/{version}", get(challenge_image))
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Accept a redirect target ONLY if it's a same-origin absolute PATH — leading `/`, not `//`
/// (protocol-relative), and no control chars / whitespace (header-injection / open-redirect
/// guard). Anything else falls back to `/`.
fn validate_redir(redir: Option<&str>) -> String {
    match redir {
        Some(r)
            if r.starts_with('/')
                && !r.starts_with("//")
                && r.chars().all(|c| !c.is_control() && !c.is_whitespace()) =>
        {
            r.to_string()
        }
        _ => "/".to_string(),
    }
}

// ---- The interstitial page ----------------------------------------------------------------

#[derive(Template)]
#[template(path = "challenge.html")]
struct InterstitialTemplate {
    /// Where to send the client after they pay the toll (validated same-origin path).
    redir: String,
}

/// Render the toll interstitial as a `429` (self-contained — no nav/DB, like `error_page.html`).
/// `X-Robots-Tag: noindex` so a false-positived crawler never indexes the toll. Public so the
/// enforcement middleware (CX.5) serves the SAME page for a greylisted request.
pub fn render_interstitial(redir: &str) -> Response {
    let tmpl = InterstitialTemplate {
        redir: validate_redir(Some(redir)),
    };
    let robots = HeaderName::from_static("x-robots-tag");
    match tmpl.render() {
        Ok(html) => (
            StatusCode::TOO_MANY_REQUESTS,
            [
                (robots, "noindex, nofollow"),
                (header::RETRY_AFTER, "120"),
            ],
            Html(html),
        )
            .into_response(),
        Err(_) => (
            StatusCode::TOO_MANY_REQUESTS,
            "Bot toll required, but the page failed to render. Reload to try again.",
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct InterstitialQuery {
    redir: Option<String>,
}

async fn show_interstitial(Query(q): Query<InterstitialQuery>) -> Response {
    render_interstitial(&validate_redir(q.redir.as_deref()))
}

// ---- GET /challenge/new — issue a signed token (O(1), no store, no rate-limit) -------------

#[derive(Serialize)]
struct NewChallenge {
    inner_seed: String,
    ts: i64,
    version: String,
    seed: String,
    width: u32,
    height: u32,
    image_url: String,
}

async fn new_challenge(State(state): State<AppState>) -> Result<Response, AppError> {
    let ch = &state.challenge;
    let mut inner = [0u8; 16];
    openssl::rand::rand_bytes(&mut inner)?;
    let ts = Utc::now().timestamp();
    let version = ch.toll.version.clone();
    let seed = derive_seed(
        &ch.key,
        &ChallengeParams {
            inner_seed: &inner,
            ts,
            version: &version,
        },
    )?;
    Ok(Json(NewChallenge {
        inner_seed: b64(&inner),
        ts,
        version: version.clone(),
        seed: b64(&seed),
        width: ch.toll.width,
        height: ch.toll.height,
        image_url: format!("/challenge/image/{version}"),
    })
    .into_response())
}

// ---- GET /challenge/image/{version} — the raw-RGBA toll buffer (immutable, versioned) ------

async fn challenge_image(State(state): State<AppState>, Path(version): Path<String>) -> Response {
    let ch = &state.challenge;
    if version != ch.toll.version {
        // A stale version (post-deploy art swap) — the client re-fetches /challenge/new.
        return (StatusCode::NOT_FOUND, "unknown toll image version").into_response();
    }
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ),
        ],
        ch.toll.rgba.clone(),
    )
        .into_response()
}

// ---- GET /challenge/verify — verify the answer, mint clearance, redirect -------------------

#[derive(Deserialize)]
struct VerifyQuery {
    inner_seed: String,
    ts: i64,
    version: String,
    answer: String,
    redir: Option<String>,
    /// Client-reported solve time in ms (best-effort signal).
    ms: Option<i64>,
}

async fn verify_challenge(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Query(q): Query<VerifyQuery>,
) -> Result<Response, AppError> {
    let ch = &state.challenge;
    let redir = validate_redir(q.redir.as_deref());
    // Any failure path re-serves the toll (a fresh solve), never a dead end.
    let retry = || redirect(&format!("/challenge?redir={}", urlencode(&redir)), None);

    if q.version != ch.toll.version {
        return Ok(retry()); // art rotated across a deploy
    }
    let (Ok(inner), Ok(answer)) = (
        URL_SAFE_NO_PAD.decode(q.inner_seed.as_bytes()),
        URL_SAFE_NO_PAD.decode(q.answer.as_bytes()),
    ) else {
        return Ok(retry());
    };

    let now = Utc::now().timestamp();
    let ok = verify_answer(
        &ch.key,
        &ChallengeParams {
            inner_seed: &inner,
            ts: q.ts,
            version: &q.version,
        },
        &ch.toll.digest,
        &answer,
        now,
        FRESHNESS_WINDOW,
    )?;
    if !ok {
        return Ok(retry());
    }

    // Record the solve (best-effort signal; a failure here must not deny a valid clearance).
    let ip = peer.ip().to_string();
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    if let Err(e) = GreylistDao::record_clearance(&state.pool, &ip, q.ms, None, ua).await {
        tracing::warn!("failed to record toll clearance for {ip}: {e}");
    }

    let expiry = now + CLEARANCE_TTL_DAYS * 86_400;
    let cookie = mint_clearance(&ch.key, expiry)?;
    Ok(redirect(&redir, Some(&clearance_cookie(&cookie))))
}

/// Build the `Set-Cookie` value for the clearance: `HttpOnly` + `SameSite=Lax` always, `Secure`
/// only in release (mirrors the session cookie; the plain-HTTP test harness needs it off).
fn clearance_cookie(token: &str) -> String {
    let max_age = CLEARANCE_TTL_DAYS * 86_400;
    let secure = if cfg!(debug_assertions) { "" } else { "; Secure" };
    format!("{CLEARANCE_COOKIE}={token}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}")
}

/// A `302` to `location`, optionally setting a cookie. `location` is already-validated.
fn redirect(location: &str, set_cookie: Option<&str>) -> Response {
    match set_cookie {
        Some(cookie) => (
            StatusCode::FOUND,
            [
                (header::LOCATION, location.to_string()),
                (header::SET_COOKIE, cookie.to_string()),
            ],
        )
            .into_response(),
        None => (
            StatusCode::FOUND,
            [(header::LOCATION, location.to_string())],
        )
            .into_response(),
    }
}

/// Minimal percent-encoding for a path placed in a query value (space + the few chars that would
/// break the `redir=` param). The path is already validated to same-origin + no control/space.
fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '%' => "%25".to_string(),
            '&' => "%26".to_string(),
            '#' => "%23".to_string(),
            '?' => "%3F".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_redir_only_accepts_same_origin_paths() {
        assert_eq!(validate_redir(Some("/pages/projects")), "/pages/projects");
        assert_eq!(validate_redir(Some("/")), "/");
        // Open-redirect / injection attempts fall back to "/".
        assert_eq!(validate_redir(Some("//evil.com")), "/");
        assert_eq!(validate_redir(Some("https://evil.com")), "/");
        assert_eq!(validate_redir(Some("/a\r\nSet-Cookie: x")), "/");
        assert_eq!(validate_redir(Some("javascript:alert(1)")), "/");
        assert_eq!(validate_redir(None), "/");
    }

    #[test]
    fn interstitial_is_429_noindex_with_the_copy() {
        let resp = render_interstitial("/pages/projects");
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers().get("x-robots-tag").unwrap(),
            "noindex, nofollow"
        );
    }
}
