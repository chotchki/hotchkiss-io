use crate::{
    db::dao::{attachments::AttachmentDao, content_pages::ContentPageDao},
    web::{
        app_error::AppError, app_state::AppState, html_template::HtmlTemplate,
        htmx_responses::htmx_refresh, session::SessionData,
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use http::{header, StatusCode};
use tracing::debug;

pub fn attachments_router() -> Router<AppState> {
    Router::new()
        .route("/{:page_id}", get(list_page_attachments))
        .route("/{:page_id}", post(save_attachments))
        .layer(DefaultBodyLimit::disable())
        .route("/id/{:attachment_id}", get(load_attachment_by_id))
        .route("/{:page_id}/{:attachment_name}", get(load_attachment))
        .route("/{:page_id}/{:attachment_name}", delete(delete_attachment))
}

#[derive(Template)]
#[template(path = "pages/list_attachments.html")]
pub struct ListAttachmentsTemplate {
    pub page_id: i64,
    pub attachments: Vec<AttachmentDao>,
}

pub async fn list_page_attachments(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_id): Path<i64>,
) -> Result<Response, AppError> {
    if !session_data.auth_state.is_admin() {
        return Ok((StatusCode::FORBIDDEN, "Invalid Permission").into_response());
    }

    let attachments = AttachmentDao::find_attachments_by_page_id(&state.pool, page_id).await?;

    Ok(HtmlTemplate(ListAttachmentsTemplate {
        page_id,
        attachments,
    })
    .into_response())
}

pub async fn load_attachment(
    State(state): State<AppState>,
    Path((page_id, attachment_name)): Path<(i64, String)>,
) -> Result<Response, AppError> {
    debug!("We got here");

    let attachment =
        AttachmentDao::find_attachment_by_name(&state.pool, page_id, &attachment_name).await?;

    if let Some(a) = attachment {
        let headers = [
            (header::CONTENT_TYPE, a.mime_type),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", attachment_name),
            ),
            (
                header::CONTENT_LENGTH,
                a.attachment_content.len().to_string(),
            ),
        ];

        Ok((headers, a.attachment_content).into_response())
    } else {
        Ok((StatusCode::NOT_FOUND, "Attachment does not exist").into_response())
    }
}

pub async fn load_attachment_by_id(
    State(state): State<AppState>,
    Path(attachment_id): Path<i64>,
) -> Result<Response, AppError> {
    debug!("We got here");

    let attachment = AttachmentDao::find_attachment_by_id(&state.pool, attachment_id).await?;

    if let Some(a) = attachment {
        let headers = [
            (header::CONTENT_TYPE, a.mime_type),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", a.attachment_name),
            ),
            (
                header::CONTENT_LENGTH,
                a.attachment_content.len().to_string(),
            ),
        ];

        Ok((headers, a.attachment_content).into_response())
    } else {
        Ok((StatusCode::NOT_FOUND, "Attachment does not exist").into_response())
    }
}

pub async fn save_attachments(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_id): Path<i64>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    if !session_data.auth_state.is_admin() {
        return Ok((StatusCode::FORBIDDEN, "Invalid Permission").into_response());
    }

    let cp = ContentPageDao::find_by_id(&state.pool, page_id)
        .await?
        .ok_or_else(|| anyhow!("Can't attach to a non existent page"))?;

    while let Some(field) = multipart.next_field().await? {
        let name = field
            .file_name()
            .ok_or_else(|| anyhow!("Files need names!"))?
            .to_string();
        let data = field.bytes().await?;

        AttachmentDao::create(
            &state.pool,
            cp.page_id,
            name.to_string(),
            mime_guess::from_path(&name)
                .first()
                .ok_or_else(|| anyhow!("Unable to figure out mime type {}", name))?
                .to_string(),
            data.to_vec(),
        )
        .await?;
    }

    Ok(htmx_refresh())
}

pub async fn delete_attachment(
    State(state): State<AppState>,
    session_data: SessionData,
    Path((page_id, attachment_name)): Path<(i64, String)>,
) -> Result<Response, AppError> {
    if !session_data.auth_state.is_admin() {
        return Ok((StatusCode::FORBIDDEN, "Invalid Permission").into_response());
    }

    let attachment =
        AttachmentDao::find_attachment_by_name(&state.pool, page_id, &attachment_name).await?;

    if let Some(a) = attachment {
        a.delete(&state.pool).await?;

        Ok(htmx_refresh())
    } else {
        Err(anyhow!("Attachment does not exist").into())
    }
}
