use std::{net::SocketAddr, time::Instant};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::header,
    middleware::Next,
    response::Response,
};
use sqlx::SqlitePool;
use tracing::warn;

use crate::db::dao::request_log::{NewRequestLog, RequestLogDao};

/// Records every request — method, path, response status, client IP (from
/// `ConnectInfo`, if the serving stack supplies it), `User-Agent`, `Referer` —
/// to the `request_log` table. The insert is `tokio::spawn`'d fire-and-forget:
/// logging never adds latency to nor fails a response; an insert error is
/// logged and dropped.
///
/// Wired via `axum::middleware::from_fn_with_state(pool, log_requests)`.
pub async fn log_requests(State(pool): State<SqlitePool>, req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());

    let user_agent = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let referer = req
        .headers()
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    // The livereload long-poll is pure noise in dev; /admin/logs is excluded ALWAYS so
    // the log viewer never feeds the access log it tails (Phase CO). /admin/analytics is
    // excluded too (CQ.7): with the htmx control-model rework it hx-get's ITSELF on every
    // range/audience toggle, so logging it would flood request_log with the admin's own
    // dashboard traffic + pollute the very numbers it renders. (Trade-off: the dashboard
    // can't measure its OWN render latency — a minor loss vs the self-pollution.)
    // /challenge/* is the greylist toll's OWN ceremony (new / verify / image) — excluded (CY.4)
    // so a solving client's ~5 requests don't pollute top-paths. The `challenged=1` stamp lives on
    // the ORIGINAL tolled path (e.g. `/`), not here, so the signal is untouched.
    // /media/file/ is static-asset-class byte traffic, excluded since Phase DD: audio/video
    // STREAMING means hundreds-to-thousands of range requests per listen — greylist R3 fires
    // at 1000 req/IP/24h (calibrated on ~366-request human days), so two family listeners
    // behind one home NAT would plausibly greylist their OWN IP, and the rows would swamp the
    // Humans audience + top-paths signal (covers/responsive images already spray this path on
    // every page view). Honest cost: no byte-route analytics. The embed/302/content routes
    // still log, so listens are visible as page views.
    #[cfg(debug_assertions)]
    let skip = path.starts_with("/tower-livereload")
        || path.starts_with("/admin/logs")
        || path.starts_with("/admin/analytics")
        || path.starts_with("/challenge")
        || path.starts_with("/media/file/");
    #[cfg(not(debug_assertions))]
    let skip = path.starts_with("/admin/logs")
        || path.starts_with("/admin/analytics")
        || path.starts_with("/challenge")
        || path.starts_with("/media/file/");

    // SERVER-handler processing time — the inner stack + handler, measured at the
    // outermost log layer. NOT client page-load/LCP (no TLS/network/download), and it
    // under-counts streaming bodies (ServeFile returns before the last byte). A failed
    // cast saturates to 0, never i64::MAX — a bogus huge outlier would poison the
    // latency percentiles/max the CQ dashboard computes off this column.
    let start = Instant::now();
    let response = next.run(req).await;
    let duration_ms = i64::try_from(start.elapsed().as_millis()).unwrap_or(0);

    if !skip {
        // A greylist-tolled response carries the `Challenged` marker (CX.5). A challenged
        // request is provably-not-human, so force `is_bot` regardless of the UA — that's the
        // whole point (UA-spoofing scrapers finally classify correctly).
        let challenged = response
            .extensions()
            .get::<crate::web::middleware::greylist_challenge::Challenged>()
            .is_some();
        // Stamp the bot classification at write (CR.2) so the dashboard's audience
        // filter is a cheap indexed count, not a per-row 25-LIKE scan.
        let is_bot = challenged || crate::db::dao::request_log::is_bot(user_agent.as_deref());
        let entry = NewRequestLog {
            method,
            path,
            status: i64::from(response.status().as_u16()),
            ip,
            user_agent,
            referer,
            duration_ms,
            is_bot,
            challenged,
        };
        tokio::spawn(async move {
            if let Err(e) = RequestLogDao::insert(&pool, &entry).await {
                warn!("failed to record request to request_log: {e}");
            }
        });
    }

    response
}
