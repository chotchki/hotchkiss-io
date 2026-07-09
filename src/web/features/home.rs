//! The landing page (Phase 13) — the featured front door, replacing the old
//! `/` → first-content-page redirect.
//!
//! The identity/hero (headshot + name + tagline) renders site-wide from
//! `base.html`, so this owns only the content block: a one-line "what I do" +
//! GitHub/email links, three pillar DOORS (Projects / Writing / Résumé — the live,
//! distinct destinations; Software+3D share the `/projects` tree until the Phase-15
//! gallery), and a self-maintaining **Latest** strip. "Latest" is AUTO — newest
//! across `blog` + `projects`, the same fetch the unified feed uses — so the front
//! door freshens itself on every publish with no curation step and no new schema.

use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate,
        markdown::render_cache::cached_excerpt, session::SessionData,
    },
};
use askama::Template;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
};

/// How many recent items the "Latest" strip shows.
const LATEST_TOTAL: usize = 6;
/// How many children to pull per section before partitioning. Generous so a
/// FEATURED pin on an older item is still found (Featured isn't recency-limited).
/// A pinned item beyond the newest-N-per-section won't surface — moot at
/// personal-site scale; a `WHERE page_category LIKE` query is the scale lever.
const PER_SECTION_FETCH: i64 = 100;

/// One content card (used by both the Featured band and the Latest strip).
/// `section`/`section_label` drive the fallback icon + badge; `href` is the
/// section's real detail route (blog → `/blog/<slug>`, projects →
/// `/pages/projects/<slug>`).
pub struct ContentCard {
    pub section: &'static str,
    pub section_label: &'static str,
    pub href: String,
    pub title: String,
    pub date: String,
    pub cover_url: Option<String>,
    pub excerpt: String,
    /// Future-dated (scheduled/draft) — admin-only, drives the "Scheduled" badge.
    pub is_scheduled: bool,
    /// The min_role gate's badge label (from the fail-closed decode; None =
    /// public, no badge) — renders beside the Scheduled pill.
    pub visibility: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    /// Pinned showpieces (the `featured` category tag), newest-first, ALL of them.
    pub featured: Vec<ContentCard>,
    /// Newest NON-featured content (so a pinned item isn't shown twice).
    pub latest: Vec<ContentCard>,
    pub meta: crate::web::features::seo::Meta,
}

async fn card_from(state: &AppState, section: &'static str, page: &ContentPageDao) -> ContentCard {
    let (section_label, href) = match section {
        "blog" => ("Blog", format!("/blog/{}", page.page_name)),
        _ => ("Project", format!("/pages/projects/{}", page.page_name)),
    };
    ContentCard {
        section,
        section_label,
        href,
        title: page.display_title(),
        date: page.page_creation_date.format("%B %-d, %Y").to_string(),
        cover_url: crate::web::features::media::cover_url_for(&state.pool, page.page_id).await,
        excerpt: cached_excerpt(&page.page_markdown),
        is_scheduled: page.is_scheduled(),
        visibility: page.visibility_label(),
    }
}

pub async fn show_home(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    // Newest across both content sections, merged. Mirrors the unified feed's
    // sort (creation date DESC, page_id DESC tiebreak) so ordering agrees.
    let mut rows: Vec<(&'static str, ContentPageDao)> = Vec::new();
    let viewer = session_data.auth_state.role();
    for section in ["blog", "projects"] {
        if let Some(parent) = ContentPageDao::find_by_name(&state.pool, None, section).await? {
            // Section gate (DA): a min_role on the section's special row drops
            // the WHOLE section from the home bands — the per-row retain below
            // checks child rows only and would miss an ancestor gate.
            if !parent.is_visible_to(viewer) {
                continue;
            }
            let children = ContentPageDao::find_by_parent_newest_first(
                &state.pool,
                Some(parent.page_id),
                Some(PER_SECTION_FETCH),
            )
            .await?;
            for page in children {
                rows.push((section, page));
            }
        }
    }
    rows.sort_by(|a, b| {
        b.1.page_creation_date
            .cmp(&a.1.page_creation_date)
            .then(b.1.page_id.cmp(&a.1.page_id))
    });

    // Visibility gate (Phase CU scheduling + Phase DA min_role): drop hidden pages
    // before the Featured/Latest split so a scheduled OR role-gated post — even a
    // pinned one — never surfaces on the front door to an insufficient viewer;
    // admin sees scheduled ones inline (badged) to preview placement, and the
    // LATEST_TOTAL cap then counts only visible items.
    rows.retain(|(_, p)| p.is_visible_to(viewer));

    // Split pinned → Featured, the rest → Latest. `rows` is newest-first, so Latest
    // (the auto tail) keeps that order. Featured is HAND-CURATED, so re-sort it by
    // the manual `page_order` ASC — the same control the /projects drag-reorder sets
    // — with a recency tiebreak (page_order is per-parent, so a blog↔project tie
    // falls back to date then id, deterministically). Building a card fetches its
    // cover, so only the Latest we'll SHOW get built.
    let (mut featured_rows, rest): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|(_, p)| p.is_featured());
    featured_rows.sort_by(|a, b| {
        a.1.page_order
            .cmp(&b.1.page_order)
            .then(b.1.page_creation_date.cmp(&a.1.page_creation_date))
            .then(b.1.page_id.cmp(&a.1.page_id))
    });

    let mut featured: Vec<ContentCard> = Vec::with_capacity(featured_rows.len());
    for (section, page) in &featured_rows {
        featured.push(card_from(&state, section, page).await);
    }
    let mut latest: Vec<ContentCard> = Vec::new();
    for (section, page) in &rest {
        if latest.len() >= LATEST_TOTAL {
            break;
        }
        latest.push(card_from(&state, section, page).await);
    }

    // Home canonical is the site root (`canonical_path` = "").
    let meta = crate::web::features::seo::Meta::section(
        &state.site_host,
        "Christopher Hotchkiss".to_string(),
        "Full-stack software and 3D-printed hardware. This whole site is one of the \
         builds — self-hosted Rust, its own DNS and TLS."
            .to_string(),
        "",
    );

    let template = HomeTemplate {
        top_bar: TopBar::create(&state.pool, "home", viewer).await?,
        auth_state: session_data.auth_state,
        featured,
        latest,
        meta,
    };
    Ok(HtmlTemplate(template).into_response())
}
