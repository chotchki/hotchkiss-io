//! Greylist management (Phase CX.7): view active entries + recent clearances, manually pin an IP,
//! or release one. Admin-gated by the `/admin` nest's `require_admin`. Pin/release update the
//! in-memory snapshot IMMEDIATELY (not just the DB), so the toll starts/stops without waiting for
//! the next sweep refresh.

use std::net::IpAddr;

use askama::Template;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Form,
};
use http::StatusCode;
use serde::Deserialize;

use crate::db::dao::greylist::{CandidatePath, GreylistDao};
use crate::greylist::detection::is_signature_path;
use crate::web::{
    app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
    features::top_bar::TopBar, html_template::HtmlTemplate, htmx_responses::htmx_refresh,
    session::SessionData,
};

const TS_FMT: &str = "%Y-%m-%d %H:%M";

pub struct GreylistRow {
    pub ip: String,
    pub reason: String,
    pub evidence: String,
    pub manual: bool,
    pub updated_at: String,
    /// Formatted expiry, or "never (pinned)" for a manual pin.
    pub expires_at: String,
}

pub struct ClearanceRow {
    pub ip: String,
    pub cleared_at: String,
    pub solve: String,
    pub user_agent: String,
}

#[derive(Template)]
#[template(path = "admin/greylist.html")]
pub struct GreylistTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub entries: Vec<GreylistRow>,
    pub clearances: Vec<ClearanceRow>,
    /// Total tolls served (all time) — the "it's working" number.
    pub challenged_count: i64,
    /// Never-succeeded paths greylisted IPs probe that R1 doesn't match yet (CX.9) — candidates to
    /// add to `SIGNATURE_PATTERNS`.
    pub candidates: Vec<CandidatePath>,
}

pub async fn show_greylist(
    State(state): State<AppState>,
    session: SessionData,
) -> Result<Response, AppError> {
    let entries = GreylistDao::active(&state.pool)
        .await?
        .into_iter()
        .map(|e| GreylistRow {
            ip: e.ip,
            reason: e.reason,
            evidence: e.evidence.unwrap_or_default(),
            manual: e.manual,
            updated_at: e.updated_at.format(TS_FMT).to_string(),
            expires_at: e
                .expires_at
                .map(|x| x.format(TS_FMT).to_string())
                .unwrap_or_else(|| "never (pinned)".to_string()),
        })
        .collect();

    let clearances = GreylistDao::recent_clearances(&state.pool, 50)
        .await?
        .into_iter()
        .map(|c| ClearanceRow {
            ip: c.ip,
            cleared_at: c.cleared_at.format(TS_FMT).to_string(),
            solve: c.solve_ms.map(|ms| format!("{ms} ms")).unwrap_or_default(),
            user_agent: c.user_agent.unwrap_or_default(),
        })
        .collect();

    let challenged_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM request_log WHERE challenged = 1")
            .fetch_one(&state.pool)
            .await?;

    // Candidate signatures: never-succeeded paths greylisted IPs probe that R1 doesn't cover yet.
    let candidates: Vec<CandidatePath> = GreylistDao::candidate_signatures(&state.pool, 100)
        .await?
        .into_iter()
        .filter(|c| !is_signature_path(&c.path))
        .take(25)
        .collect();

    Ok(HtmlTemplate(GreylistTemplate {
        top_bar: TopBar::create(&state.pool, "admin", session.auth_state.role()).await?,
        auth_state: session.auth_state,
        entries,
        clearances,
        challenged_count,
        candidates,
    })
    .into_response())
}

#[derive(Deserialize)]
pub struct PinForm {
    pub ip: String,
}

/// `POST /admin/greylist/pin` — manually greylist an IP (never lapses until released). Also
/// inserts into the in-memory snapshot so the toll bites on the client's very next request.
pub async fn pin_ip(
    State(state): State<AppState>,
    Form(form): Form<PinForm>,
) -> Result<Response, AppError> {
    let ip = form.ip.trim();
    if ip.parse::<IpAddr>().is_err() {
        return Ok((StatusCode::BAD_REQUEST, "Not a valid IP address").into_response());
    }
    GreylistDao::pin_manual(&state.pool, ip, "manual").await?;
    state.greylist.insert(ip);
    Ok(htmx_refresh())
}

/// `POST /admin/greylist/{ip}/release` — remove an IP from the greylist (a false positive, or a
/// pin you're done with). Removes it from the snapshot too, so the toll stops immediately.
pub async fn release_ip(
    State(state): State<AppState>,
    Path(ip): Path<String>,
) -> Result<Response, AppError> {
    GreylistDao::release(&state.pool, &ip).await?;
    state.greylist.remove(&ip);
    Ok(htmx_refresh())
}

/// `POST /admin/greylist/run-sweep` — force a detection pass NOW instead of waiting for the 15-min
/// timer. Release-safe (no `debug_assertions` seam), so it works on beta: `curl` a signature path
/// a couple times, click this, and your IP appears greylisted. Refreshes the page to show the
/// result; the counts are logged.
pub async fn run_sweep(State(state): State<AppState>) -> Result<Response, AppError> {
    let cache = crate::greylist::crawler::CrawlerCache::new(std::time::Duration::from_secs(60));
    let report =
        crate::greylist::sweep::run_once(&state.pool, &state.resolver, &cache, &state.greylist)
            .await?;
    tracing::info!("manual greylist sweep: {report:?}");
    Ok(htmx_refresh())
}
