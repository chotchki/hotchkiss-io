use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::{
            listing::{paginate, ListOrder, ListingQuery, Pagination},
            not_found,
            pages::{EditQuery, GetPageTemplate, PostNavCard},
            top_bar::TopBar,
        },
        html_template::HtmlTemplate,
        markdown::{
            render_cache::{cached_excerpt, cached_transform},
            title::strip_leading_h1,
        },
        session::SessionData,
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

pub fn blog_router() -> Router<AppState> {
    Router::new()
        .route("/", get(show_index))
        // The feed is unified (blog + projects) and lives canonically at
        // `/feed.xml`; this path serves the same handler for back-compat.
        .route("/feed.xml", get(crate::web::features::feed::show_feed))
        .route("/{slug}", get(show_post))
}

pub struct BlogPostCard {
    pub page_name: String,
    pub title: String,
    pub page_creation_date: String,
    pub cover_url: Option<String>,
    pub excerpt: String,
    /// Future-dated (scheduled/draft) — admin-only, drives the "Scheduled" badge.
    pub is_scheduled: bool,
}

#[derive(Template)]
#[template(path = "blog/index.html")]
pub struct BlogIndexTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub posts: Vec<BlogPostCard>,
    pub pagination: Pagination,
    pub meta: crate::web::features::seo::Meta,
}

pub async fn show_index(
    State(state): State<AppState>,
    session_data: SessionData,
    Query(query): Query<ListingQuery>,
) -> Result<Response, AppError> {
    let blog_page = ContentPageDao::find_by_name(&state.pool, None, "blog").await?;
    let Some(blog_page) = blog_page else {
        return Err(
            anyhow!("Server misconfiguration, could not find the `blog` special page").into(),
        );
    };

    let is_admin = session_data.auth_state.is_admin();
    let (raw_posts, pagination) = paginate(
        &state.pool,
        Some(blog_page.page_id),
        &query,
        ListOrder::Newest,
        "/blog",
        is_admin,
    )
    .await?;

    let mut posts: Vec<BlogPostCard> = Vec::with_capacity(raw_posts.len());
    for p in raw_posts {
        let cover_url = crate::web::features::media::cover_url_for(&state.pool, p.page_id).await;
        let is_scheduled = p.is_scheduled();
        posts.push(BlogPostCard {
            title: p.display_title(),
            page_name: p.page_name,
            page_creation_date: p.page_creation_date.format("%B %-d, %Y").to_string(),
            cover_url,
            excerpt: cached_excerpt(&p.page_markdown),
            is_scheduled,
        });
    }

    let meta = crate::web::features::seo::Meta::section(
        &state.site_host,
        "Blog — Christopher Hotchkiss".to_string(),
        "Writing from Christopher Hotchkiss on software, hardware, and building things.".to_string(),
        "blog",
    );

    let template = BlogIndexTemplate {
        top_bar: TopBar::create(&state.pool, "blog").await?,
        auth_state: session_data.auth_state,
        posts,
        pagination,
        meta,
    };
    Ok(HtmlTemplate(template).into_response())
}

pub async fn show_post(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(slug): Path<String>,
    Query(edit_q): Query<EditQuery>,
) -> Result<Response, AppError> {
    let pages_path = ContentPageDao::find_by_path(&state.pool, &["blog", &slug]).await?;

    // Scheduled/timed publishing gate (Phase CU): a future-dated post 404s for
    // non-admins through the SAME cat-404 a genuine miss returns, so a non-admin
    // can't tell "scheduled" from "nonexistent". is_admin is computed before
    // auth_state moves into the template.
    let is_admin = session_data.auth_state.is_admin();
    let Some(lp) = pages_path.last() else {
        return Ok(not_found::render_not_found(&state.pool, session_data.auth_state).await);
    };
    if !pages_path.iter().all(|n| n.is_visible_to(is_admin)) {
        return Ok(not_found::render_not_found(&state.pool, session_data.auth_state).await);
    }

    // Next/previous nav: pull the post's siblings in the SAME newest-first order
    // the index uses, drop any the viewer can't see (a scheduled post must not
    // surface as a Prev/Next card; admin keeps all), locate this post, take its
    // neighbors. Previous = older (one step down the list), Next = newer (one step
    // up). A side is None at the ends, so the oldest/newest post shows only one card.
    let mut siblings =
        ContentPageDao::find_by_parent_newest_first(&state.pool, lp.parent_page_id, None).await?;
    siblings.retain(|p| p.is_visible_to(is_admin));
    let (prev_post, next_post) = match siblings.iter().position(|p| p.page_id == lp.page_id) {
        Some(i) => (
            siblings.get(i + 1).map(nav_card),
            i.checked_sub(1).and_then(|j| siblings.get(j)).map(nav_card),
        ),
        None => (None, None),
    };

    let cover_url = crate::web::features::media::cover_url_for(&state.pool, lp.page_id).await;
    let meta = crate::web::features::seo::Meta::page(
        &state.site_host,
        lp.display_title(),
        &lp.page_markdown,
        &format!("blog/{slug}"),
        cover_url.as_deref(),
        "article",
    );

    let gpt = GetPageTemplate {
        top_bar: TopBar::create(&state.pool, "blog").await?,
        auth_state: session_data.auth_state,
        page_path: format!("blog/{slug}"),
        page: lp.clone(),
        pages_path: pages_path.clone(),
        children_pages: ContentPageDao::find_by_parent(&state.pool, Some(lp.page_id)).await?,
        rendered_markdown: cached_transform(&strip_leading_h1(&lp.page_markdown))?,
        edit: edit_q.edit.is_some(),
        prev_post,
        next_post,
        pdf_url: None,
        cover_media_ref: crate::web::features::media::cover_ref_for(&state.pool, lp.page_id).await,
        meta,
        posted_date: Some(lp.page_creation_date.format("%B %-d, %Y").to_string()),
        hero: crate::web::features::media::cover_hero_for(&state.pool, lp.page_id).await,
        hero_overlay: edit_q.hero.as_deref() == Some("overlay"),
    };
    Ok(HtmlTemplate(gpt).into_response())
}

/// Map a post row to the compact card the next/previous nav renders.
fn nav_card(p: &ContentPageDao) -> PostNavCard {
    PostNavCard {
        page_name: p.page_name.clone(),
        title: p.display_title(),
        page_creation_date: p.page_creation_date.format("%B %-d, %Y").to_string(),
    }
}
