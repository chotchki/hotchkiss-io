use crate::web::htmx_responses::htmx_redirect;
use crate::web::util::deserialize::empty_string_as_none;
use crate::web::util::slug::slugify;
use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        html_template::HtmlTemplate, htmx_responses::htmx_refresh,
        markdown::{links::rewrite_site_links, title::strip_leading_h1, transformer::transform},
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

pub mod attachments;
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

    if let Some(f) = titles.first() {
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
            if lp.special_page {
                return Ok(Redirect::temporary(&lp.page_markdown).into_response());
            }

            let gpt = GetPageTemplate {
                top_bar: TopBar::create(&state.pool, page_names.first().unwrap()).await?,
                auth_state: session_data.auth_state,
                page_path: page_path.clone(),
                page: lp.clone(),
                pages_path: pages_path.clone(),
                children_pages: ContentPageDao::find_by_parent(&state.pool, Some(lp.page_id))
                    .await?,
                rendered_markdown: transform(&strip_leading_h1(&lp.page_markdown))?,
                edit: edit_q.edit.is_some(),
                prev_post: None,
                next_post: None,
                pdf_url: None,
                cover_media_ref: crate::web::features::media::cover_ref_for(
                    &state.pool,
                    lp.page_id,
                )
                .await,
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
}

pub async fn put_page_path(
    State(state): State<AppState>,
    Path(page_path): Path<String>,
    Form(put_page_form): Form<PutPageForm>,
) -> Result<Response, AppError> {
    // The cover is now a media ref (BZ.8); resolve it → media_id. An empty or
    // unknown ref clears the cover.
    let cover_media_id: Option<i64> = match &put_page_form.page_cover_media_ref {
        Some(r) => crate::db::dao::media::MediaDao::find_by_ref(&state.pool, r)
            .await?
            .map(|m| m.media_id),
        None => None,
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
            lp.update(&state.pool).await?;

            sqlx::query!(
                r#"UPDATE content_pages SET page_cover_media_id = ?1 WHERE page_id = ?2"#,
                cover_media_id,
                lp.page_id
            )
            .execute(&state.pool)
            .await?;

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
            cp.update(&state.pool).await?;

            Ok(htmx_redirect(&format!("/pages/{page_path}/{slug}?edit=1"))?)
        }
        None => Ok((StatusCode::NOT_FOUND, "No such parent page").into_response()),
    }
}
