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
            listing::{paginate, ListOrder, ListingQuery, Pagination},
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
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

pub fn library_router() -> Router<AppState> {
    Router::new()
        .route("/", get(show_library_index))
        .route("/audiobooks", get(show_audiobooks))
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
    /// Slug for the admin edit link (`/pages/library/<page_name>?edit`) —
    /// sections are real authored pages, and the index is the only place an
    /// admin ever sees them (the /library routes shadow the content view).
    pub page_name: String,
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

/// A book card on `/library/audiobooks` — mirrors the blog card, linking into
/// the content tree where `get_page_path` serves the detail page.
pub struct BookCard {
    pub page_name: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub excerpt: String,
    /// Admin-only badges (an insufficient viewer never reaches this page).
    pub is_scheduled: bool,
    pub visibility: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "library/audiobooks.html")]
pub struct AudiobooksTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub books: Vec<BookCard>,
    pub pagination: Pagination,
    /// The `audiobooks` child page exists (chris has authored the section) —
    /// false renders the not-set-up-yet empty state instead of "no results".
    pub section_exists: bool,
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
            page_name: p.page_name.clone(),
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

pub async fn show_audiobooks(
    State(state): State<AppState>,
    session_data: SessionData,
    Query(query): Query<ListingQuery>,
) -> Result<Response, AppError> {
    let library = match gate(&state, &session_data, "/library/audiobooks").await? {
        Ok(row) => row,
        Err(gate_response) => return Ok(gate_response),
    };
    let viewer = session_data.auth_state.role();

    // The `audiobooks` section is an AUTHORED child of `library` (not seeded —
    // inherit-on-create stamps its min_role from the parent). Missing just
    // means chris hasn't set it up yet: render the empty state, not a 500.
    let section =
        ContentPageDao::find_by_name(&state.pool, Some(library.page_id), "audiobooks").await?;
    let section = section.filter(|s| s.is_visible_to(viewer));

    let (rows, pagination) = match &section {
        Some(s) => {
            paginate(
                &state.pool,
                Some(s.page_id),
                &query,
                ListOrder::Newest,
                "/library/audiobooks",
                viewer,
            )
            .await?
        }
        None => (
            Vec::new(),
            Pagination {
                current_page: 1,
                total_pages: 1,
                total_results: 0,
                search: String::new(),
                base_path: "/library/audiobooks".to_string(),
            },
        ),
    };

    let mut books: Vec<BookCard> = Vec::with_capacity(rows.len());
    for p in &rows {
        books.push(BookCard {
            page_name: p.page_name.clone(),
            title: p.display_title(),
            cover_url: crate::web::features::media::cover_url_for(&state.pool, p.page_id).await,
            excerpt: cached_excerpt(&p.page_markdown),
            is_scheduled: p.is_scheduled(),
            visibility: p.visibility_label(),
        });
    }

    let template = AudiobooksTemplate {
        top_bar: TopBar::create(&state.pool, "library", viewer).await?,
        auth_state: session_data.auth_state,
        books,
        pagination,
        section_exists: section.is_some(),
    };
    Ok(HtmlTemplate(template).into_response())
}
