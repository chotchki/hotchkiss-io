use crate::web::{
    app_error::AppError, app_state::AppState, markdown::transformer::transform,
    session::SessionData,
};
use axum::{
    response::{IntoResponse, Response},
    routing::patch,
    Form, Router,
};
use http::StatusCode;
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
) -> Result<Response, AppError> {
    if !session_data.auth_state.is_admin() {
        return Ok((StatusCode::FORBIDDEN, "Invalid Permission").into_response());
    }

    Ok(transform(&page_markdown.markdown)?.into_response())
}
