//! `/3d` — the 3D-printing gallery index (Phase CW, gallery half).
//!
//! Lists the children of the `3d` special page as model cards: a **Featured** band
//! (the pinned showpieces — the SAME Pin button / `featured` tag the landing uses,
//! but scoped here) above the rest. Model detail pages live under the content tree
//! at `/pages/3d/<slug>` and are served by the ordinary `get_page_path`, so this
//! module owns only the index. 3D never appears on `/` — `show_home` only fetches
//! `blog` + `projects`, so a `featured`-tagged model surfaces ONLY here.
//!
//! Later this root hosts the WASM slicer/placer editor (CW.1–4); the nesting is
//! unchanged when it lands.

use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate,
        markdown::render_cache::cached_excerpt, session::SessionData,
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

pub fn three_d_router() -> Router<AppState> {
    Router::new().route("/", get(show_3d_index))
}

/// A model card for the `/3d` gallery — cover render, title, excerpt — linking to
/// the model's detail page at `/pages/3d/<slug>`. Mirrors the project card.
pub struct ModelCard {
    pub page_name: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub excerpt: String,
    /// Future-dated (scheduled) — admin-only, drives the "Scheduled" badge (CU).
    pub is_scheduled: bool,
}

#[derive(Template)]
#[template(path = "3d/index.html")]
pub struct ThreeDIndexTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    /// Pinned showpieces (the `featured` tag), `page_order`-sorted.
    pub featured: Vec<ModelCard>,
    /// The rest of the (published) models, in manual `page_order`.
    pub models: Vec<ModelCard>,
    pub meta: crate::web::features::seo::Meta,
}

async fn card_from(state: &AppState, page: &ContentPageDao) -> ModelCard {
    ModelCard {
        title: page.display_title(),
        page_name: page.page_name.clone(),
        cover_url: crate::web::features::media::cover_url_for(&state.pool, page.page_id).await,
        excerpt: cached_excerpt(&page.page_markdown),
        is_scheduled: page.is_scheduled(),
    }
}

pub async fn show_3d_index(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let three_d = ContentPageDao::find_by_name(&state.pool, None, "3d").await?;
    let Some(three_d) = three_d else {
        return Err(anyhow!("Server misconfiguration, could not find the `3d` special page").into());
    };

    // Children in manual page_order (drag-reorder like /projects).
    let mut rows = ContentPageDao::find_by_parent(&state.pool, Some(three_d.page_id)).await?;
    // Scheduled/timed publishing gate (CU): hide future-dated models from non-admins.
    let is_admin = session_data.auth_state.is_admin();
    rows.retain(|p| p.is_visible_to(is_admin));

    // Pinned → Featured (page_order-sorted, recency-tiebroken like the landing);
    // the rest below. Reuses the exact Pin/`featured` mechanism, scoped to 3D.
    let (mut featured_rows, rest): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|p| p.is_featured());
    featured_rows.sort_by(|a, b| {
        a.page_order
            .cmp(&b.page_order)
            .then(b.page_creation_date.cmp(&a.page_creation_date))
            .then(b.page_id.cmp(&a.page_id))
    });

    let mut featured: Vec<ModelCard> = Vec::with_capacity(featured_rows.len());
    for p in &featured_rows {
        featured.push(card_from(&state, p).await);
    }
    let mut models: Vec<ModelCard> = Vec::with_capacity(rest.len());
    for p in &rest {
        models.push(card_from(&state, p).await);
    }

    let meta = crate::web::features::seo::Meta::section(
        &state.site_host,
        "3D — Christopher Hotchkiss".to_string(),
        "3D-printed hardware and OpenSCAD designs by Christopher Hotchkiss — the physical half of the portfolio.".to_string(),
        "3d",
    );

    let template = ThreeDIndexTemplate {
        top_bar: TopBar::create(&state.pool, "3d").await?,
        auth_state: session_data.auth_state,
        featured,
        models,
        meta,
    };
    Ok(HtmlTemplate(template).into_response())
}
