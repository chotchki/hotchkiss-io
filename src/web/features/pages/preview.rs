use crate::{
    db::dao::roles::Role,
    web::{
        app_error::AppError,
        app_state::AppState,
        markdown::transformer::transform,
        session::{AuthenticationState, SessionData},
    },
};
use anyhow::anyhow;
use axum::{routing::patch, Form, Router};
use serde::Deserialize;

pub fn preview_router() -> Router<AppState> {
    Router::new().route("/preview", patch(preview_page))
}

#[derive(Debug, Deserialize)]
pub struct PreviewForm {
    markdown: String,
}

pub async fn preview_page(
    session_data: SessionData,
    Form(page_markdown): Form<PreviewForm>,
) -> Result<String, AppError> {
    if let AuthenticationState::Authenticated(user) = &session_data.auth_state {
        if user.role != Role::Admin {
            return Err(anyhow!("Invalid Permission").into());
        }
    }

    Ok(transform(&page_markdown.markdown)?)
}
