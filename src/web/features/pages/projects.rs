use crate::{
    db::dao::{content_pages::ContentPageDao, projects::ProjectDao, roles::Role},
    web::{
        app_error::AppError,
        app_state::AppState,
        features::top_bar::TopBar,
        html_template::HtmlTemplate,
        markdown::transformer::transform,
        session::{AuthenticationState, SessionData},
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, patch, put},
    Form, Router,
};
use http::HeaderMap;
use serde::Deserialize;

pub fn projects_router() -> Router<AppState> {
    Router::new().route("/", get(show_all_projects))
    //.route("/{:page_name}", get(page_by_title))
    //.route("/{:page_name}", put(edit_page))
    //.route("/{:page_name}", delete(delete_page))
    //.route("/{:page_name}/preview", patch(preview_page))
}

#[derive(Template)]
#[template(path = "projects/list_projects.html")]
pub struct ListProjectsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub projects: Vec<ProjectDao>,
}

pub async fn show_all_projects(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<HtmlTemplate<ListProjectsTemplate>, AppError> {
    let projects = ProjectDao::get_projects_in_order(&state.pool).await?;

    let top_bar =
        TopBar::new(ContentPageDao::find_page_titles(&state.pool).await?).make_active("projects");

    let lpt = ListProjectsTemplate {
        top_bar,
        auth_state: session_data.auth_state,
        projects,
    };
    Ok(HtmlTemplate(lpt))
}
