use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
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

pub fn projects_router() -> Router<AppState> {
    Router::new().route("/", get(show_all_projects))
}

/// A project card for the `/projects` index. Mirrors the blog card (cover or
/// fallback icon, display title, excerpt) so the two indexes read the same — but
/// without a date, since projects aren't chronological the way posts are.
pub struct ProjectCard {
    pub page_name: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub excerpt: String,
    /// Future-dated (scheduled/draft) — admin-only, drives the "Scheduled" badge.
    pub is_scheduled: bool,
    /// The min_role gate's badge label (from the fail-closed decode; None =
    /// public, no badge) — renders beside the Scheduled pill.
    pub visibility: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "projects/list_projects.html")]
pub struct ListProjectsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub projects: Vec<ProjectCard>,
    pub pagination: Pagination,
    pub meta: crate::web::features::seo::Meta,
}

pub async fn show_all_projects(
    State(state): State<AppState>,
    session_data: SessionData,
    Query(query): Query<ListingQuery>,
) -> Result<Response, AppError> {
    let project_page = ContentPageDao::find_by_name(&state.pool, None, "projects").await?;
    let Some(project_page) = project_page else {
        return Err(
            anyhow!("Server misconfiguration, could not find the /projects special page").into(),
        );
    };

    let viewer = session_data.auth_state.role();
    // Section gate (DA): a min_role on the `projects` special row darkens the
    // code route too — same cat-404 as a genuine miss (see blog::show_index).
    if !project_page.is_visible_to(viewer) {
        return Ok(crate::web::features::not_found::render_not_found(
            &state.pool,
            session_data.auth_state,
        )
        .await);
    }
    let (raw_projects, pagination) = paginate(
        &state.pool,
        Some(project_page.page_id),
        &query,
        ListOrder::Ordered,
        "/projects",
        viewer,
    )
    .await?;
    let mut projects: Vec<ProjectCard> = Vec::with_capacity(raw_projects.len());
    for p in raw_projects {
        let cover_url = crate::web::features::media::cover_url_for(&state.pool, p.page_id).await;
        let is_scheduled = p.is_scheduled();
        let visibility = p.visibility_label();
        projects.push(ProjectCard {
            title: p.display_title(),
            page_name: p.page_name,
            cover_url,
            excerpt: cached_excerpt(&p.page_markdown),
            is_scheduled,
            visibility,
        });
    }

    let meta = crate::web::features::seo::Meta::section(
        &state.site_host,
        "Projects — Christopher Hotchkiss".to_string(),
        "Software and hardware projects by Christopher Hotchkiss — public, clickable proof of range."
            .to_string(),
        "projects",
    );

    let lpt = ListProjectsTemplate {
        top_bar: TopBar::create(&state.pool, "projects", viewer).await?,
        auth_state: session_data.auth_state,
        projects,
        pagination,
        meta,
    };
    Ok(HtmlTemplate(lpt).into_response())
}
