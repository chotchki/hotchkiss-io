use std::collections::HashSet;

use crate::{
    db::dao::{
        content_pages::{self, get_page_by_name, save, ContentPage},
        roles::Role,
    },
    web::{
        app_error::AppError,
        app_state::AppState,
        html_template::HtmlTemplate,
        session::{AuthenticationState, SessionData},
    },
};
use anyhow::{anyhow, Result};
use askama::Template;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, patch, post, put},
    Form, Json, Router,
};
use http::HeaderMap;
use serde::Deserialize;

use super::top_bar::TopBar;

pub fn pages_router() -> Router<AppState> {
    Router::new()
        .route("/", get(default_page))
        .route("/edit", get(edit_pages_view))
        .route("/edit", patch(reorder_pages))
        .route("/edit", put(create_page))
        .route("/{:page_name}", get(page_by_title))
        .route("/{:page_name}", put(edit_page))
        .route("/{:page_name}", delete(delete_page))
        .route("/{:page_name}/preview", patch(preview_page))
}

#[derive(Template)]
#[template(path = "content_page.html")]
pub struct PagesTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub page_name: String,
    pub rendered_markdown: String,
}

pub async fn default_page(State(state): State<AppState>) -> Result<Redirect, AppError> {
    let titles = content_pages::find_page_titles(&state.pool).await?;

    match titles.first() {
        Some(f) => Ok(Redirect::temporary(&format!("/pages/{f}"))),
        None => Err(anyhow!("No pages exist, this is a server config error").into()),
    }
}

#[derive(Template)]
#[template(path = "edit_pages.html")]
pub struct EditPagesTemplate {
    pub top_bar: TopBar,
    pub pages: Vec<(String, bool)>,
    pub auth_state: AuthenticationState,
}

pub async fn edit_pages_view(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<HtmlTemplate<EditPagesTemplate>, AppError> {
    if let AuthenticationState::Authenticated(ref user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Err(anyhow!("Invalid Permission").into());
        }
    }

    let top_bar = TopBar::new(content_pages::find_page_titles(&state.pool).await?);

    let pages = content_pages::find_page_titles_and_special(&state.pool).await?;

    Ok(HtmlTemplate(EditPagesTemplate {
        top_bar,
        pages,
        auth_state: session_data.auth_state,
    }))
}

pub async fn reorder_pages(
    State(state): State<AppState>,
    session_data: SessionData,
    Json(title_order): Json<Vec<String>>,
) -> Result<(), AppError> {
    if let AuthenticationState::Authenticated(ref user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Err(anyhow!("Invalid Permission").into());
        }
    }

    let pages = content_pages::find_page_titles_and_special(&state.pool).await?;
    let page_titles: Vec<String> = pages.into_iter().map(|(t, _)| t).collect();

    let title_order_set: HashSet<&String> = title_order.iter().collect();
    let page_titles_set: HashSet<&String> = page_titles.iter().collect();

    if title_order_set != page_titles_set {
        return Err(anyhow!("Missing pages, cannot reorder").into());
    }

    //Now we can reorder the pages in the database
    let mut transaction = state.pool.begin().await?;
    for (i, title) in title_order.iter().enumerate() {
        let mut page = get_page_by_name(&mut *transaction, title)
            .await?
            .ok_or_else(|| anyhow!("Unable to load page to reorder"))?;

        page.page_order = i64::try_from(i)?;
        save(&mut *transaction, &page).await?;
    }
    transaction.commit().await?;

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct CreatePageForm {
    page_name: String,
}

pub async fn create_page(
    State(state): State<AppState>,
    session_data: SessionData,
    Form(form): Form<CreatePageForm>,
) -> Result<impl IntoResponse, AppError> {
    if let AuthenticationState::Authenticated(user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Err(anyhow!("Invalid Permission").into());
        }
    }

    let page = get_page_by_name(&state.pool, &form.page_name).await?;
    if page.is_some() {
        return Err(anyhow!("Page Already Exists").into());
    }

    let cp = ContentPage {
        page_name: form.page_name,
        page_markdown: "".to_string(),
        page_order: 0,
        special_page: false,
    };

    save(&state.pool, &cp).await?;

    let mut headers = HeaderMap::new();
    headers.insert("HX-Refresh", "true".parse()?);

    Ok(headers)
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
        rendered_markdown: markdown::to_html(&page.page_markdown),
    };

    Ok(HtmlTemplate(pt).into_response())
}

pub async fn delete_page(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_name): Path<String>,
) -> Result<(), AppError> {
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

    Ok(())
}

pub async fn edit_page(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_name): Path<String>,
    Json(page_markdown): Json<String>,
) -> Result<(), AppError> {
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
            page_markdown: page_markdown.clone(),
            special_page: false,
        });

    page.page_markdown = page_markdown;

    save(&state.pool, &page).await?;

    Ok(())
}

pub async fn preview_page(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_name): Path<String>,
    Json(page_markdown): Json<String>,
) -> Result<Response, AppError> {
    let top_bar =
        TopBar::new(content_pages::find_page_titles(&state.pool).await?).make_active(&page_name);

    let pt = PagesTemplate {
        top_bar,
        auth_state: session_data.auth_state,
        page_name,
        rendered_markdown: markdown::to_html(&page_markdown),
    };

    Ok(HtmlTemplate(pt).into_response())
}
