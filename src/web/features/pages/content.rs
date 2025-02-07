use crate::{
    db::dao::{
        content_pages::{self, get_page_by_name, save, ContentPage},
        roles::Role,
    },
    web::{
        app_error::AppError,
        app_state::AppState,
        features::top_bar::TopBar,
        html_template::HtmlTemplate,
        session::{AuthenticationState, SessionData},
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, patch, put},
    Form, Json, Router,
};
use http::HeaderMap;
use serde::Deserialize;

pub fn content_router() -> Router<AppState> {
    Router::new()
        .route("/", get(default_page))
        .route("/{:page_name}", get(page_by_title))
        .route("/{:page_name}", put(edit_page))
        .route("/{:page_name}", delete(delete_page))
        .route("/{:page_name}/preview", patch(preview_page))
}

#[derive(Template)]
#[template(path = "pages/content_page.html")]
pub struct PagesTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub page_name: String,
    pub markdown: String,
    pub rendered_markdown: String,
}

pub async fn default_page(State(state): State<AppState>) -> Result<Redirect, AppError> {
    let titles = content_pages::find_page_titles(&state.pool).await?;

    match titles.first() {
        Some(f) => Ok(Redirect::temporary(&format!("/pages/{f}"))),
        None => Err(anyhow!("No pages exist, this is a server config error").into()),
    }
}

pub async fn page_by_title(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_name): Path<String>,
) -> Result<Response, AppError> {
    let page = get_page_by_name(&state.pool, &page_name)
        .await?
        .ok_or_else(|| anyhow!("Unknown page"))?;

    if page.special_page {
        return Ok(Redirect::temporary(&page.page_markdown).into_response());
    }

    let top_bar =
        TopBar::new(content_pages::find_page_titles(&state.pool).await?).make_active(&page_name);

    let pt = PagesTemplate {
        top_bar,
        auth_state: session_data.auth_state,
        page_name,
        markdown: page.page_markdown.clone(),
        rendered_markdown: markdown::to_html(&page.page_markdown),
    };

    Ok(HtmlTemplate(pt).into_response())
}

pub async fn delete_page(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_name): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if let AuthenticationState::Authenticated(user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Err(anyhow!("Invalid Permission").into());
        }
    }

    let page = get_page_by_name(&state.pool, &page_name)
        .await?
        .ok_or_else(|| anyhow!("Unknown page"))?;

    if page.special_page {
        return Err(anyhow!("Cannot delete special pages").into());
    }

    content_pages::delete(&state.pool, &page_name).await?;

    let mut headers = HeaderMap::new();
    headers.insert("HX-Refresh", "true".parse()?);

    Ok(headers)
}

#[derive(Debug, Deserialize)]
pub struct EditForm {
    markdown: String,
}

pub async fn edit_page(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_name): Path<String>,
    Form(page_markdown): Form<EditForm>,
) -> Result<impl IntoResponse, AppError> {
    if let AuthenticationState::Authenticated(user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Err(anyhow!("Invalid Permission").into());
        }
    }

    let mut page = get_page_by_name(&state.pool, &page_name)
        .await?
        .unwrap_or_else(|| ContentPage {
            page_name,
            page_order: 0,
            page_markdown: page_markdown.markdown.clone(),
            special_page: false,
        });

    page.page_markdown = page_markdown.markdown;

    save(&state.pool, &page).await?;

    let mut headers = HeaderMap::new();
    headers.insert("HX-Refresh", "true".parse()?);

    Ok(headers)
}

#[derive(Debug, Deserialize)]
pub struct PreviewForm {
    markdown: String,
}

pub async fn preview_page(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_name): Path<String>,
    Form(page_markdown): Form<PreviewForm>,
) -> Result<Response, AppError> {
    let top_bar =
        TopBar::new(content_pages::find_page_titles(&state.pool).await?).make_active(&page_name);

    let pt = PagesTemplate {
        top_bar,
        auth_state: session_data.auth_state,
        page_name,
        markdown: page_markdown.markdown.clone(),
        rendered_markdown: markdown::to_html(&page_markdown.markdown),
    };

    Ok(HtmlTemplate(pt).into_response())
}
