use crate::web::{
    app_error::AppError, app_state::AppState, markdown::transformer::transform,
};
use axum::{
    response::{IntoResponse, Response},
    routing::patch,
    Form, Router,
};
use serde::Deserialize;

pub fn preview_router() -> Router<AppState> {
    Router::new().route("/preview", patch(preview_page))
}

#[derive(Debug, Deserialize)]
pub struct PreviewForm {
    page_markdown: String,
}

pub async fn preview_page(
    Form(page_markdown): Form<PreviewForm>,
) -> Result<Response, AppError> {
    Ok(transform(&page_markdown.page_markdown)?.into_response())
}
