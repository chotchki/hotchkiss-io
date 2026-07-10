//! Dead-link admin surface (Phase DL.7): broken links in site content, grouped by
//! the page that references them, with a global "Run scan now" + a per-link
//! re-check. Admin-gated by the `/admin` nest's `require_admin`. Like analytics /
//! greylist, this reads the DB the daily scan populates.

use std::collections::BTreeMap;

use askama::Template;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Form,
};
use serde::Deserialize;
use sqlx::types::chrono::Utc;

use crate::db::dao::content_pages::ContentPageDao;
use crate::deadlinks::{LinkCheckDao, LinkCheckRow, LinkRefDao, ReqwestChecker};
use crate::web::{
    app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
    features::top_bar::TopBar, html_template::HtmlTemplate, htmx_responses::htmx_refresh,
    session::SessionData,
};

const TS_FMT: &str = "%Y-%m-%d %H:%M";

/// Which review bucket a problem link is in — drives the badge + sort order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeadLinkBucket {
    Confirmed,
    Failing,
    Review,
}

impl DeadLinkBucket {
    fn from_row(r: &LinkCheckRow) -> Self {
        if r.is_confirmed_dead() {
            DeadLinkBucket::Confirmed
        } else if r.is_failing() {
            DeadLinkBucket::Failing
        } else {
            DeadLinkBucket::Review
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DeadLinkBucket::Confirmed => "Confirmed dead",
            DeadLinkBucket::Failing => "Failing",
            DeadLinkBucket::Review => "Review",
        }
    }

    /// Severity for sorting (worst first).
    pub fn rank(self) -> u8 {
        match self {
            DeadLinkBucket::Confirmed => 3,
            DeadLinkBucket::Failing => 2,
            DeadLinkBucket::Review => 1,
        }
    }

    /// Tailwind classes for the badge.
    pub fn badge_class(self) -> &'static str {
        match self {
            DeadLinkBucket::Confirmed => "bg-red-700 text-white",
            DeadLinkBucket::Failing => "bg-yellow text-navy",
            DeadLinkBucket::Review => "bg-navy/20 text-navy",
        }
    }
}

#[derive(Clone)]
pub struct DeadLinkItem {
    pub url: String,
    pub kind: &'static str,
    pub bucket: DeadLinkBucket,
    /// A short human note — "HTTP 404", "no such page or media", "timeout".
    pub status: String,
    pub streak: i64,
    pub last_checked: String,
}

pub struct DeadLinkPageGroup {
    pub title: String,
    pub edit_url: String,
    pub items: Vec<DeadLinkItem>,
}

#[derive(Template)]
#[template(path = "admin/dead_links.html")]
pub struct DeadLinksTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub running: bool,
    pub last_checked: String,
    pub confirmed: usize,
    pub failing: usize,
    pub review: usize,
    pub total_tracked: i64,
    pub ok_count: i64,
    pub groups: Vec<DeadLinkPageGroup>,
}

pub async fn show_dead_links(
    State(state): State<AppState>,
    session: SessionData,
) -> Result<Response, AppError> {
    let rows = LinkCheckDao::problem_rows(&state.pool).await?;

    // Group each problem link under every page that references it (a url cited by
    // two pages shows under both — each page owns its own fix).
    let mut by_page: BTreeMap<i64, Vec<DeadLinkItem>> = BTreeMap::new();
    for row in &rows {
        let item = item_from(row);
        for page_id in LinkRefDao::pages_for_url(&state.pool, &row.url).await? {
            by_page.entry(page_id).or_default().push(item.clone());
        }
    }

    let mut groups: Vec<DeadLinkPageGroup> = Vec::new();
    for (page_id, mut items) in by_page {
        items.sort_by_key(|i| std::cmp::Reverse(i.bucket.rank()));
        let (title, edit_url) = page_link(&state.pool, page_id).await?;
        groups.push(DeadLinkPageGroup {
            title,
            edit_url,
            items,
        });
    }
    // Worst-affected pages first, then by title.
    groups.sort_by(|a, b| {
        let ra = a.items.iter().map(|i| i.bucket.rank()).max().unwrap_or(0);
        let rb = b.items.iter().map(|i| i.bucket.rank()).max().unwrap_or(0);
        rb.cmp(&ra).then_with(|| a.title.cmp(&b.title))
    });

    let confirmed = rows.iter().filter(|r| r.is_confirmed_dead()).count();
    let failing = rows.iter().filter(|r| r.is_failing()).count();
    let review = rows.iter().filter(|r| r.needs_review()).count();
    let (total_tracked, ok_count) = LinkCheckDao::counts(&state.pool).await?;
    let last_checked = LinkCheckDao::last_checked(&state.pool)
        .await?
        .map(|t| t.format(TS_FMT).to_string())
        .unwrap_or_else(|| "never".to_string());

    Ok(HtmlTemplate(DeadLinksTemplate {
        top_bar: TopBar::create(&state.pool, "admin", session.auth_state.role()).await?,
        auth_state: session.auth_state,
        running: state.dead_links.status().running,
        last_checked,
        confirmed,
        failing,
        review,
        total_tracked,
        ok_count,
        groups,
    })
    .into_response())
}

/// `POST /admin/dead-links/run-scan` — kick off a full scan. A scan does external
/// HTTP + can take minutes, so it's SPAWNED (fire-and-return); the page shows
/// "scan running" and a later refresh shows the result. No-op if one's already
/// running (the single-flight guard). Release-safe (no debug seam).
pub async fn run_scan(State(state): State<AppState>) -> Result<Response, AppError> {
    crate::deadlinks::trigger_now(
        state.pool.clone(),
        state.site_host.clone(),
        state.dead_links.clone(),
    );
    Ok(htmx_refresh())
}

#[derive(Deserialize)]
pub struct RecheckForm {
    pub url: String,
}

/// `POST /admin/dead-links/recheck` — re-check ONE url synchronously (bounded by
/// the request timeout), so a false positive can be cleared (or a fix confirmed)
/// without re-scanning the whole site. On `ok` the streak resets immediately.
pub async fn recheck(
    State(state): State<AppState>,
    Form(form): Form<RecheckForm>,
) -> Result<Response, AppError> {
    let checker = ReqwestChecker::new()?;
    crate::deadlinks::recheck_one(
        &state.pool,
        &checker,
        &state.site_host,
        form.url.trim(),
        Utc::now(),
    )
    .await?;
    Ok(htmx_refresh())
}

fn item_from(row: &LinkCheckRow) -> DeadLinkItem {
    DeadLinkItem {
        url: row.url.clone(),
        kind: row.kind().as_str(),
        bucket: DeadLinkBucket::from_row(row),
        status: row
            .detail
            .clone()
            .unwrap_or_else(|| row.class().as_str().to_string()),
        streak: row.consecutive_failures,
        last_checked: row.last_checked_at.format(TS_FMT).to_string(),
    }
}

/// Build a page's display title + its `?edit=1` editor URL by walking parents to the
/// root. A deleted page (raced with the scan) degrades to the Manage-Pages list.
async fn page_link(pool: &sqlx::SqlitePool, page_id: i64) -> Result<(String, String), AppError> {
    let Some(page) = ContentPageDao::find_by_id(pool, page_id).await? else {
        return Ok(("(deleted page)".to_string(), "/admin/pages".to_string()));
    };
    let title = page.display_title();
    let mut names = vec![page.page_name.clone()];
    let mut parent = page.parent_page_id;
    while let Some(pid) = parent {
        match ContentPageDao::find_by_id(pool, pid).await? {
            Some(p) => {
                names.push(p.page_name.clone());
                parent = p.parent_page_id;
            }
            None => break,
        }
    }
    names.reverse();
    Ok((title, format!("/pages/{}?edit=1", names.join("/"))))
}
