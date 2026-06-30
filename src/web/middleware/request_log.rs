use std::net::SocketAddr;

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

    // The livereload long-poll is pure noise in dev; /admin/logs is excluded
    // ALWAYS so the log viewer never feeds the access log it tails (Phase CO —
    // the no-self-feed guard against an infinite-loop-ish growth).
    #[cfg(debug_assertions)]
    let skip = path.starts_with("/tower-livereload") || path.starts_with("/admin/logs");
    #[cfg(not(debug_assertions))]
    let skip = path.starts_with("/admin/logs");

    let response = next.run(req).await;

    if !skip {
        let entry = NewRequestLog {
            method,
            path,
            status: i64::from(response.status().as_u16()),
            ip,
            user_agent,
            referer,
        };
        tokio::spawn(async move {
            if let Err(e) = RequestLogDao::insert(&pool, &entry).await {
                warn!("failed to record request to request_log: {e}");
            }
        });
    }

    response
}
