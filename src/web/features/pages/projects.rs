use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate, markdown::excerpt::excerpt,
        session::SessionData,
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

pub fn projects_router() -> Router<AppState> {
    Router::new().route("/", get(show_all_projects))
}

/// A project card for the `/projects` index. Mirrors the blog card (cover or
/// fallback icon, display title, excerpt) so the two indexes read the same — but
/// without a date, since projects aren't chronological the way posts are.
pub struct ProjectCard {
    pub page_name: String,
    pub title: String,
    pub page_cover_attachment_id: Option<i64>,
    pub excerpt: String,
}

#[derive(Template)]
#[template(path = "projects/list_projects.html")]
pub struct ListProjectsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub projects: Vec<ProjectCard>,
}

pub async fn show_all_projects(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let project_page = ContentPageDao::find_by_name(&state.pool, None, "projects").await?;
    let Some(project_page) = project_page else {
        return Err(
            anyhow!("Server misconfiguration, could not find the /projects special page").into(),
        );
    };

    let projects: Vec<ProjectCard> =
        ContentPageDao::find_by_parent(&state.pool, Some(project_page.page_id))
            .await?
            .into_iter()
            .map(|p| ProjectCard {
                title: p.display_title(),
                page_name: p.page_name,
                page_cover_attachment_id: p.page_cover_attachment_id,
                excerpt: excerpt(&p.page_markdown),
            })
            .collect();

    let lpt = ListProjectsTemplate {
        top_bar: TopBar::create(&state.pool, "projects").await?,
        auth_state: session_data.auth_state,
        projects,
    };
    Ok(HtmlTemplate(lpt).into_response())
}
