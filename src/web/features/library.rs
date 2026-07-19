//! `/library` — the Family library (Phase DE, the first consumer of the
//! role-gate foundation).
//!
//! The `/3d` shape: this module owns only the INDEX routes — `/library`
//! (section doors from the special page's children) and `/library/audiobooks`
//! (paginated book cards via `listing.rs`). Book detail pages live under the
//! content tree at `/pages/library/audiobooks/<slug>` and are served by the
//! ordinary `get_page_path` (whose ancestor scan cat-404s them for
//! insufficient viewers — DATA stays miss-shaped).
//!
//! CODE-DEFINED routes deliberately do NOT miss-shape: route names ship in the
//! public source mirror, so a cat-404 here buys nothing, and a session-expired
//! bookmark that 404s is a support call from mom. Instead an insufficient
//! viewer gets a state-aware sign-in gate (see `render_gate`). The gate is
//! driven by the `library` special row's OWN `min_role` — the migration seeds
//! `Family`, and re-stamping the row (editor Visibility) retunes the section
//! with zero code changes.

use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError,
        app_state::AppState,
        authentication_state::AuthenticationState,
        features::{
            child_index,
            listing::{ListOrder, ListingQuery},
            top_bar::TopBar,
        },
        html_template::HtmlTemplate,
        markdown::render_cache::cached_excerpt,
        session::SessionData,
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};

pub fn library_router() -> Router<AppState> {
    Router::new()
        .route("/", get(show_library_index))
        // ONE generic section route (Phase DV): audiobooks, manga, and any future
        // section render through it — gate + the shared child-index listing widget.
        .route("/{section}", get(show_library_section))
}

/// The state-aware sign-in gate for insufficient viewers on code-defined
/// `/library` routes. Two copy states: logged-out → "sign in" plus a login
/// link carrying `?next=<this route>`; authenticated-but-insufficient (a
/// self-registered stranger — they exist by construction) → a neutral
/// "restricted" with NO tier names and NO sign-in loop.
#[derive(Template)]
#[template(path = "library/gate.html")]
pub struct LibraryGateTemplate {
    pub top_bar: TopBar,
    /// Also picks the copy state via `is_authenticated()` in the template.
    pub auth_state: AuthenticationState,
    /// URL-encoded login href for the logged-out state: `/login?next=…`.
    pub login_href: String,
}

/// A section door on the `/library` index — one per child page (audiobooks
/// now; a future manga/video section is just a new child page).
pub struct SectionDoor {
    /// Door target: `/library/<page_name>` (the audiobooks child resolves to
    /// the code route; future sections get routes when they're built).
    pub href: String,
    pub title: String,
    pub excerpt: String,
}

#[derive(Template)]
#[template(path = "library/index.html")]
pub struct LibraryIndexTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub doors: Vec<SectionDoor>,
}

/// A library SECTION index (audiobooks, manga, …) — the section title + its
/// children rendered by the shared child-index widget (DV.7). The `grid` is the
/// pre-rendered (safe) listing HTML; the `edit_href` is the admin section-editor link.
#[derive(Template)]
#[template(path = "library/section.html")]
pub struct LibrarySectionTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub title: String,
    /// The children grid HTML (cards + search + pager), rendered by `child_index`.
    pub grid: String,
    pub is_admin: bool,
    /// `/pages/library/<section>?edit` — the admin "edit section" target.
    pub edit_href: String,
}

/// Load the `library` special row and gate the viewer against ITS `min_role`.
/// `Ok(row)` = come on in; `Err(gate response)` = the caller returns it as-is.
async fn gate(
    state: &AppState,
    session_data: &SessionData,
    next: &str,
) -> Result<Result<ContentPageDao, Response>, AppError> {
    let library = ContentPageDao::find_by_name(&state.pool, None, "library")
        .await?
        .ok_or_else(|| {
            anyhow!("Server misconfiguration, could not find the `library` special page")
        })?;

    let viewer = session_data.auth_state.role();
    if library.is_visible_to(viewer) {
        return Ok(Ok(library));
    }

    let template = LibraryGateTemplate {
        // The gate page renders the normal nav — which correctly does NOT
        // show the Library tab to this viewer (is_nav_visible_to).
        top_bar: TopBar::create(&state.pool, "library", viewer).await?,
        auth_state: session_data.auth_state.clone(),
        login_href: format!(
            "/login?next={}",
            crate::web::util::urlencode::urlencode(next)
        ),
    };
    Ok(Err(HtmlTemplate(template).into_response()))
}

pub async fn show_library_index(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let library = match gate(&state, &session_data, "/library").await? {
        Ok(row) => row,
        Err(gate_response) => return Ok(gate_response),
    };
    let viewer = session_data.auth_state.role();

    let mut children =
        ContentPageDao::find_by_parent(&state.pool, Some(library.page_id)).await?;
    children.retain(|p| p.is_visible_to(viewer));

    let doors = children
        .iter()
        .map(|p| SectionDoor {
            href: format!("/library/{}", p.page_name),
            title: p.display_title(),
            excerpt: cached_excerpt(&p.page_markdown),
        })
        .collect();

    let template = LibraryIndexTemplate {
        top_bar: TopBar::create(&state.pool, "library", viewer).await?,
        auth_state: session_data.auth_state,
        doors,
    };
    Ok(HtmlTemplate(template).into_response())
}

/// A library SECTION index — `/library/<section>` (audiobooks, manga, …). ONE
/// generic handler (DV.7): gate against the `library` row, then render the section's
/// children through the shared child-index listing widget. Replaces the bespoke
/// `show_audiobooks` — audiobooks + manga are now identical (only the authored
/// content + its books/series differ). A section is an AUTHORED child of `library`
/// (inherit-on-create stamps its gate); a missing/invisible one → back to the index
/// (a bogus `/library/<x>` isn't a content 404 — this is a code route).
pub async fn show_library_section(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(section): Path<String>,
    Query(query): Query<ListingQuery>,
) -> Result<Response, AppError> {
    let route = format!("/library/{section}");
    let library = match gate(&state, &session_data, &route).await? {
        Ok(row) => row,
        Err(gate_response) => return Ok(gate_response),
    };
    let viewer = session_data.auth_state.role();

    let Some(section_page) =
        ContentPageDao::find_by_name(&state.pool, Some(library.page_id), &section)
            .await?
            .filter(|s| s.is_visible_to(viewer))
    else {
        return Ok(Redirect::temporary("/library").into_response());
    };

    // The listing/selection widget — the ONE renderer shared with the ` ```children `
    // fence. The pager stays on the code route (`route`), while the cards + the
    // new-child form target the content tree where child pages live (`child_base`),
    // so a card links to `/pages/library/<section>/<slug>` (served by get_page_path)
    // and "+ new child" POSTs `/pages/library/<section>` (the child-create path).
    let child_base = format!("/pages/library/{section}");
    let grid = child_index::render_children_grid(
        &state.pool,
        section_page.page_id,
        &query,
        ListOrder::Newest,
        &route,
        &child_base,
        viewer,
    )
    .await?;

    let template = LibrarySectionTemplate {
        top_bar: TopBar::create(&state.pool, "library", viewer).await?,
        auth_state: session_data.auth_state.clone(),
        title: section_page.display_title(),
        grid,
        is_admin: session_data.auth_state.is_admin(),
        edit_href: format!("{child_base}?edit"),
    };
    Ok(HtmlTemplate(template).into_response())
}
