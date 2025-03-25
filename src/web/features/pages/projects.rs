use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate, session::SessionData,
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

#[derive(Template)]
#[template(path = "projects/list_projects.html")]
pub struct ListProjectsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub page_path: String,
    pub projects: Vec<ContentPageDao>,
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

    let projects = ContentPageDao::find_by_parent(&state.pool, Some(project_page.page_id)).await?;

    let lpt = ListProjectsTemplate {
        top_bar: TopBar::create(&state.pool, "projects").await?,
        auth_state: session_data.auth_state,
        page_path: "/projects".to_string(),
        projects,
    };
    Ok(HtmlTemplate(lpt).into_response())
}
