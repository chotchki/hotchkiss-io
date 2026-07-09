use crate::web::htmx_responses::htmx_redirect;
use crate::web::util::deserialize::empty_string_as_none;
use sqlx::types::chrono::{DateTime, NaiveDateTime, Utc};
use crate::web::util::slug::slugify;
use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        html_template::HtmlTemplate, htmx_responses::htmx_refresh,
        markdown::{
            links::rewrite_site_links, render_cache::cached_transform, title::strip_leading_h1,
        },
        session::SessionData,
    },
};
use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use http::StatusCode;
use preview::preview_router;
use serde::Deserialize;
use tracing::debug;

use super::{not_found, top_bar::TopBar};

pub mod preview;
pub mod projects;

pub fn pages_router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(redirect_to_first_page).post(post_top_level_page_path),
        )
        .route(
            "/{*page_path}",
            get(get_page_path)
                .delete(delete_page_path)
                .put(put_page_path)
                .post(post_page_path),
        )
        .merge(preview_router())
}

pub async fn redirect_to_first_page(State(state): State<AppState>) -> Result<Response, AppError> {
    let titles = ContentPageDao::find_by_parent(&state.pool, None).await?;

    // Skip a scheduled (future-dated) first page: redirecting an anon onto its slug
    // leaks the draft's existence via the Location header — an oracle, the same leak
    // the nav-tab hide closes. Unconditional (like the nav): admins reach a scheduled
    // top-level page by direct URL / Manage Pages, not this convenience redirect
    // (Phase CU, caught in review).
    if let Some(f) = titles
        .iter()
        .find(|p| p.is_visible_to(crate::db::dao::roles::Role::Anonymous))
    {
        Ok(Redirect::temporary(&format!("/pages/{}", f.page_name)).into_response())
    } else {
        Ok((
            StatusCode::NOT_FOUND,
            "No pages found, the server has major issues",
        )
            .into_response())
    }
}

/// Compact card for the next/previous nav at the bottom of a blog post. Lives
/// on GetPageTemplate as Options that ONLY `show_post` (blog) fills — `/pages`
/// leaves them None, so the nav is blog-only by construction.
pub struct PostNavCard {
    pub page_name: String,
    pub title: String,
    pub page_creation_date: String,
}

#[derive(Template)]
#[template(path = "pages/get_page.html")]
pub struct GetPageTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub page_path: String,
    pub page: ContentPageDao,
    pub pages_path: Vec<ContentPageDao>,
    pub children_pages: Vec<ContentPageDao>,
    pub rendered_markdown: String,
    /// Admin editor visible? Driven by `?edit` so admin defaults to the clean
    /// reader view and opts into the editor.
    pub edit: bool,
    /// Adjacent blog posts (Previous = older, Next = newer). Both None on
    /// `/pages`; the template renders the nav only when one is Some.
    pub prev_post: Option<PostNavCard>,
    pub next_post: Option<PostNavCard>,
    /// `/resume.pdf` download link — Some only on the résumé page (the template
    /// shows the button when set); None on /pages and /blog.
    pub pdf_url: Option<String>,
    /// Current cover's media ref (token), pre-filling the editor's cover field
    /// (BZ.8 — covers are media now, not `page_cover_attachment_id`).
    pub cover_media_ref: Option<String>,
    /// Per-page SEO/social metadata (description, canonical, OpenGraph) rendered
    /// into `<head>` via the `{% block meta %}` override.
    pub meta: crate::web::features::seo::Meta,
    /// The post date as a human string, shown under the title — `Some` only for
    /// blog posts (`None` on `/pages` + `/resume`, which aren't dated).
    pub posted_date: Option<String>,
    /// The cover rendered as a hero banner at the top of the reader view (Phase
    /// CV) — `Some` when the page has an image cover, `None` otherwise + on the
    /// résumé. Reader-view only (not shown in the editor).
    pub hero: Option<crate::web::features::media::CoverHero>,
}

/// `?edit` (any value) toggles the admin editor on a page view; absent = the
/// clean reader view.
#[derive(Debug, Deserialize)]
pub struct EditQuery {
    pub edit: Option<String>,
}

pub async fn get_page_path(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_path): Path<String>,
    Query(edit_q): Query<EditQuery>,
) -> Result<Response, AppError> {
    let page_names: Vec<&str> = page_path.split("/").collect();
    debug!("Resolving page path {:?}", page_names);

    let pages_path = ContentPageDao::find_by_path(&state.pool, &page_names).await?;

    match pages_path.last() {
        // The human "/pages/<slug>" miss (e.g. the dead /pages/Resume link) →
        // the shared cat 404, not a bare string. The mutation handlers below
        // keep their plain "No such page" (htmx/admin responses, not nav).
        None => Ok(not_found::render_not_found(&state.pool, session_data.auth_state).await),
        Some(lp) => {
            // Scheduled/timed publishing gate (Phase CU): hide a future-dated page
            // — and the WHOLE subtree under a future-dated non-special ancestor,
            // since a leaf-only gate would still leak the hidden parent's title in
            // the breadcrumb — behind the SAME cat-404 a genuine miss returns (so an
            // insufficient viewer can't tell "scheduled"/"role-gated" from "doesn't
            // exist"). The viewer role is computed before auth_state is moved into
            // the template below.
            let viewer = session_data.auth_state.role();
            if !pages_path.iter().all(|n| n.is_visible_to(viewer)) {
                // Special-leaf wrinkle (Phase DE): nav tabs link /pages/<name>,
                // and a SPECIAL page's only possible is_visible_to failure is
                // the ROLE clause (the special_page conjunct exempts scheduling)
                // — so when every ancestor passes and only the special leaf
                // fails, issue the redirect anyway and let the target code
                // route (e.g. /library) show its state-aware sign-in gate. A
                // cat-404 here would read as a broken bookmark to a logged-out
                // family member. DATA pages keep the miss shape below.
                let ancestors_visible = pages_path[..pages_path.len() - 1]
                    .iter()
                    .all(|n| n.is_visible_to(viewer));
                if lp.special_page && ancestors_visible {
                    return Ok(Redirect::temporary(&lp.page_markdown).into_response());
                }
                return Ok(not_found::render_not_found(&state.pool, session_data.auth_state).await);
            }

            if lp.special_page {
                return Ok(Redirect::temporary(&lp.page_markdown).into_response());
            }

            let cover_url =
                crate::web::features::media::cover_url_for(&state.pool, lp.page_id).await;
            let meta = crate::web::features::seo::Meta::page(
                &state.site_host,
                lp.display_title(),
                &lp.page_markdown,
                &format!("pages/{page_path}"),
                cover_url.as_deref(),
                "article",
            );

            let gpt = GetPageTemplate {
                top_bar: TopBar::create(&state.pool, page_names.first().unwrap(), viewer).await?,
                auth_state: session_data.auth_state,
                page_path: page_path.clone(),
                page: lp.clone(),
                pages_path: pages_path.clone(),
                children_pages: ContentPageDao::find_by_parent(&state.pool, Some(lp.page_id))
                    .await?,
                rendered_markdown: cached_transform(&strip_leading_h1(&lp.page_markdown))?,
                edit: edit_q.edit.is_some(),
                prev_post: None,
                next_post: None,
                pdf_url: None,
                cover_media_ref: crate::web::features::media::cover_ref_for(
                    &state.pool,
                    lp.page_id,
                )
                .await,
                meta,
                posted_date: None,
                hero: crate::web::features::media::cover_hero_for(&state.pool, lp.page_id).await,
            };

            Ok(HtmlTemplate(gpt).into_response())
        }
    }
}

pub async fn delete_page_path(
    State(state): State<AppState>,
    Path(page_path): Path<String>,
) -> Result<Response, AppError> {
    let page_names: Vec<&str> = page_path.split("/").collect();
    let pages_path = ContentPageDao::find_by_path(&state.pool, &page_names).await?;

    match pages_path.last() {
        Some(lp) => {
            if lp.special_page {
                return Ok(
                    (StatusCode::FORBIDDEN, "Special pages cannot be deleted").into_response()
                );
            }

            lp.delete(&state.pool).await?;

            //Since the page is gone we can only send you to the parent page
            let (_, parent_paths) = page_names.split_last().unwrap();
            if !parent_paths.is_empty() {
                Ok(htmx_redirect(&format!(
                    "/pages/{}",
                    parent_paths.join("/")
                ))?)
            } else {
                Ok(htmx_redirect("/pages")?)
            }
        }
        None => Ok((StatusCode::NOT_FOUND, "No such page").into_response()),
    }
}

#[derive(Debug, Deserialize)]
pub struct PutPageForm {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub page_title: Option<String>,
    #[serde(deserialize_with = "empty_string_as_none")]
    pub page_category: Option<String>,
    pub page_markdown: String,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub page_cover_media_ref: Option<String>,
    pub page_order: i64,
    /// Optional post-date override (datetime-local `YYYY-MM-DDTHH:MM[:SS]`). Empty
    /// or unparseable → keep the existing date. Lets a Wayback-recovered post be
    /// backdated to its real chronological slot.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub page_creation_date: Option<String>,
    /// Visibility select (DB.1): `"Public"` → clear the gate (NULL), a known
    /// gate role → set it. ABSENT (an old client / a test PUT without the
    /// field) or unrecognized → keep the existing gate — bad or missing input
    /// must never silently LOOSEN visibility (the cover-typo rule).
    #[serde(default)]
    pub min_role: Option<String>,
}

/// Parse a `datetime-local` form value as a UTC instant. The input carries no
/// timezone, so we store it as UTC verbatim (exact tz is irrelevant for backdating
/// an old post). `None` on a bad/empty value → a save with no override keeps the
/// existing date.
fn parse_local_datetime(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .ok()
        .map(|naive| naive.and_utc())
}

pub async fn put_page_path(
    State(state): State<AppState>,
    Path(page_path): Path<String>,
    Form(put_page_form): Form<PutPageForm>,
) -> Result<Response, AppError> {
    // The cover is a media ref (BZ.8). The field tolerates every copyable shape
    // (`![](/media/<ref>)`, `/media/file/<url_key>`, a bare ref) — see
    // `resolve_cover_media_id`. Three outcomes:
    //   empty field       → clear the cover
    //   resolves          → set it
    //   non-empty, no hit  → leave the cover ALONE (a typo must not wipe an
    //                        existing cover — was the silent data-loss footgun).
    let cover_update: Option<Option<i64>> = match &put_page_form.page_cover_media_ref {
        None => Some(None),
        // Some(id) → set it; None (unresolvable) → skip, preserving the existing cover.
        Some(raw) => crate::web::features::media::resolve_cover_media_id(&state.pool, raw)
            .await
            .map(Some),
    };

    let page_names: Vec<&str> = page_path.split("/").collect();
    let pages_path = ContentPageDao::find_by_path(&state.pool, &page_names).await?;

    match pages_path.to_owned().last() {
        Some(lp) => {
            let mut lp = lp.to_owned();
            lp.page_title = put_page_form.page_title;
            lp.page_category = put_page_form.page_category;
            lp.page_markdown = rewrite_site_links(&put_page_form.page_markdown, &state.site_host)?;
            lp.page_order = put_page_form.page_order;
            // Optional backdating: only override the post date when a valid value
            // is supplied; otherwise keep the existing one.
            if let Some(dt) = put_page_form
                .page_creation_date
                .as_deref()
                .and_then(parse_local_datetime)
            {
                lp.page_creation_date = dt;
            }
            // Visibility (DB.1): explicit values only; absent/unrecognized
            // keeps the existing gate. `update()` stamps page_modified_date,
            // so a visibility flip busts the feed/sitemap validators.
            match put_page_form.min_role.as_deref() {
                Some("Public") => lp.min_role = None,
                Some(v @ ("Registered" | "Family" | "Admin")) => lp.min_role = Some(v.to_string()),
                _ => {}
            }
            lp.update(&state.pool).await?;

            // Cover lives in a separate column (`page_cover_media_id`) that
            // `update()` doesn't touch; set_cover stamps `page_modified_date` too
            // so a cover change keeps the feed/sitemap validators fresh. Skip it
            // entirely on an unresolvable ref so the existing cover is preserved.
            if let Some(cover_media_id) = cover_update {
                ContentPageDao::set_cover(&state.pool, lp.page_id, cover_media_id).await?;
            }

            Ok(htmx_refresh())
        }
        None => Ok((StatusCode::NOT_FOUND, "No such page").into_response()),
    }
}

#[derive(Debug, Deserialize)]
pub struct PostPageForm {
    /// The human title typed by the author; the URL slug (page_name) is derived
    /// from it server-side via `slugify`.
    pub page_title: String,
}

pub async fn post_top_level_page_path(
    State(state): State<AppState>,
    Form(post_page_form): Form<PostPageForm>,
) -> Result<Response, AppError> {
    let title = post_page_form.page_title.trim().to_string();
    let slug = slugify(&title);
    if slug.is_empty() {
        return Ok(
            (StatusCode::BAD_REQUEST, "Title must contain letters or numbers").into_response(),
        );
    }

    let mut cp =
        ContentPageDao::create(&state.pool, None, slug.clone(), None, String::new(), None).await?;
    cp.page_title = Some(title);
    // Top-level pages are born PUBLIC — no parent to inherit a gate from.
    // Explicit (create() already returns None) so the invariant survives a
    // future create() change.
    cp.min_role = None;
    cp.update(&state.pool).await?;

    // Land the author on the new page in edit mode, not back on the list.
    Ok(htmx_redirect(&format!("/pages/{slug}?edit=1"))?)
}

pub async fn post_page_path(
    State(state): State<AppState>,
    Path(page_path): Path<String>,
    Form(post_page_form): Form<PostPageForm>,
) -> Result<Response, AppError> {
    let page_names: Vec<&str> = page_path.split("/").collect();

    let title = post_page_form.page_title.trim().to_string();
    let slug = slugify(&title);
    if slug.is_empty() {
        return Ok(
            (StatusCode::BAD_REQUEST, "Title must contain letters or numbers").into_response(),
        );
    }

    let parent_pages = ContentPageDao::find_by_path(&state.pool, &page_names).await?;
    match parent_pages.last() {
        Some(lp) => {
            let mut cp = ContentPageDao::create(
                &state.pool,
                Some(lp.page_id),
                slug.clone(),
                None,
                String::new(),
                None,
            )
            .await?;
            cp.page_title = Some(title);
            // Inherit-on-create (DB.3): a new child defaults to its parent's
            // gate. Belt AND suspenders — the ancestor scan already hides the
            // subtree, and this stamp additionally covers children born AFTER
            // the gate on any future listing surface that queries child rows
            // directly. Children created BEFORE a parent was gated keep their
            // own min_role (deliberate: the ancestor scan is the enforcement;
            // there is no retroactive downward propagation).
            cp.min_role = lp.min_role.clone();
            cp.update(&state.pool).await?;

            Ok(htmx_redirect(&format!("/pages/{page_path}/{slug}?edit=1"))?)
        }
        None => Ok((StatusCode::NOT_FOUND, "No such parent page").into_response()),
    }
}
