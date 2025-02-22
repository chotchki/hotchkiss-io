use crate::{
    db::dao::{content_pages::ContentPageDao, roles::Role},
    web::{
        app_error::AppError,
        app_state::AppState,
        html_template::HtmlTemplate,
        markdown::transformer::transform,
        session::{AuthenticationState, SessionData},
    },
};
use askama::Template;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Form, Router,
};
use http::{uri::PathAndQuery, HeaderMap, StatusCode};
use preview::preview_router;
use serde::Deserialize;

use super::top_bar::TopBar;

pub mod preview;

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

#[derive(Template)]
#[template(path = "pages/get_page.html")]
pub struct GetPageTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub page_name: String,
    pub pages_path: Vec<ContentPageDao>,
    pub markdown: String,
    pub rendered_markdown: String,
}

pub async fn get_page_path(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_path): Path<String>,
) -> Result<Response, AppError> {
    let page_names: Vec<&str> = page_path.split("/").collect();

    let pages_path = ContentPageDao::find_by_path(&state.pool, &page_names).await?;

    match pages_path.last() {
        None => Ok((StatusCode::NOT_FOUND, "No such page").into_response()),
        Some(lp) => {
            if lp.special_page {
                return Ok(Redirect::temporary(&lp.page_markdown).into_response());
            }

            let top_bar = TopBar::create(&state.pool, page_names.first().unwrap()).await?;

            let gpt = GetPageTemplate {
                top_bar,
                auth_state: session_data.auth_state,
                page_name: lp.page_name.clone(),
                pages_path: pages_path.clone(),
                markdown: lp.page_markdown.clone(),
                rendered_markdown: transform(&lp.page_markdown)?,
            };

            Ok(HtmlTemplate(gpt).into_response())
        }
    }
}

pub async fn delete_page_path(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_path): Path<String>,
) -> Result<Response, AppError> {
    if let AuthenticationState::Authenticated(user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Ok((StatusCode::FORBIDDEN, "Invalid Permission").into_response());
        }
    }

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
                Ok(
                    Redirect::temporary(&format!("/pages/{}", parent_paths.join("/")))
                        .into_response(),
                )
            } else {
                Ok(Redirect::temporary("/pages").into_response())
            }
        }
        None => Ok((StatusCode::NOT_FOUND, "No such page").into_response()),
    }
}

#[derive(Debug, Deserialize)]
pub struct PutPageForm {
    pub markdown: String,
}

pub async fn put_page_path(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_path): Path<String>,
    Form(put_page_form): Form<PutPageForm>,
) -> Result<Response, AppError> {
    if let AuthenticationState::Authenticated(user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Ok((StatusCode::FORBIDDEN, "Invalid Permission").into_response());
        }
    }

    let page_names: Vec<&str> = page_path.split("/").collect();
    let pages_path = ContentPageDao::find_by_path(&state.pool, &page_names).await?;

    match pages_path.to_owned().last() {
        Some(lp) => {
            let mut lp = lp.to_owned();
            lp.page_markdown = put_page_form.markdown;
            lp.update(&state.pool).await?;

            let mut headers = HeaderMap::new();
            headers.insert("HX-Refresh", "true".parse()?);

            Ok(headers.into_response())
        }
        None => Ok((StatusCode::NOT_FOUND, "No such page").into_response()),
    }
}

#[derive(Debug, Deserialize)]
pub struct PostPageForm {
    pub page_name: String,
    pub page_category: Option<String>,
    pub page_markdown: String,
}

pub async fn post_top_level_page_path(
    State(state): State<AppState>,
    session_data: SessionData,
    Form(post_page_form): Form<PostPageForm>,
) -> Result<Response, AppError> {
    if let AuthenticationState::Authenticated(user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Ok((StatusCode::FORBIDDEN, "Invalid Permission").into_response());
        }
    }

    if PathAndQuery::try_from(post_page_form.page_name.clone()).is_err() {
        return Ok((StatusCode::BAD_REQUEST, "Page Name must be URI safe").into_response());
    }

    ContentPageDao::create(
        &state.pool,
        None,
        post_page_form.page_name,
        post_page_form.page_category,
        post_page_form.page_markdown,
        None,
    )
    .await?;

    let mut headers = HeaderMap::new();
    headers.insert("HX-Refresh", "true".parse()?);

    Ok(headers.into_response())
}

pub async fn post_page_path(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_path): Path<String>,
    Form(post_page_form): Form<PostPageForm>,
) -> Result<Response, AppError> {
    if let AuthenticationState::Authenticated(user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Ok((StatusCode::FORBIDDEN, "Invalid Permission").into_response());
        }
    }

    let page_names: Vec<&str> = page_path.split("/").collect();

    if PathAndQuery::try_from(post_page_form.page_name.clone()).is_err() {
        return Ok((StatusCode::BAD_REQUEST, "Page Name must be URI safe").into_response());
    }

    let parent_pages = ContentPageDao::find_by_path(&state.pool, &page_names).await?;
    match parent_pages.last() {
        Some(lp) => {
            ContentPageDao::create(
                &state.pool,
                Some(lp.page_id),
                post_page_form.page_name,
                post_page_form.page_category,
                post_page_form.page_markdown,
                None,
            )
            .await?;

            let mut headers = HeaderMap::new();
            headers.insert("HX-Refresh", "true".parse()?);

            Ok(headers.into_response())
        }
        None => Ok((StatusCode::NOT_FOUND, "No such parent page").into_response()),
    }
}
