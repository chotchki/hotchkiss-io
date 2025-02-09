use crate::{
    db::dao::{attachments::AttachmentDao, content_pages::ContentPageDao, roles::Role},
    web::{
        app_error::AppError,
        app_state::AppState,
        html_template::HtmlTemplate,
        session::{AuthenticationState, SessionData},
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    extract::{Multipart, Path, State},
    response::IntoResponse,
    routing::{delete, get, put},
    Router,
};
use http::{header, HeaderMap};
use tracing::debug;

pub fn attachments_router() -> Router<AppState> {
    Router::new()
        .route("/{:page_name}", get(list_page_attachments))
        .route("/{:page_name}", put(save_attachments))
        .route("/{:page_name}/{:attachment_name}", get(load_attachment))
        .route(
            "/{:page_name}/{:attachment_name}",
            delete(delete_attachment),
        )
}

#[derive(Template)]
#[template(path = "pages/list_attachments.html")]
pub struct ListAttachmentsTemplate {
    pub parent_page: String,
    pub attachment_names: Vec<String>,
}

pub async fn list_page_attachments(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(page_name): Path<String>,
) -> Result<HtmlTemplate<ListAttachmentsTemplate>, AppError> {
    if let AuthenticationState::Authenticated(ref user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Err(anyhow!("Invalid Permission").into());
        }
    }

    let titles = AttachmentDao::find_attachment_titles_by_parent(&state.pool, &page_name).await?;

    Ok(HtmlTemplate(ListAttachmentsTemplate {
        parent_page: page_name,
        attachment_names: titles,
    }))
}

pub async fn load_attachment(
    State(state): State<AppState>,
    Path((page_name, attachment_name)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    debug!("We got here");

    let attachment =
        AttachmentDao::find_attachment(&state.pool, &page_name, &attachment_name).await?;

    if let Some(a) = attachment {
        let headers = [
            (header::CONTENT_TYPE, a.mime_type),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", attachment_name),
            ),
        ];

        Ok((headers, a.attachment_content))
    } else {
        Err(anyhow!(
            "Attachment does not exist {} {}",
            page_name,
            attachment_name
        )
        .into())
    }
}

pub async fn save_attachments(
    State(state): State<AppState>,
    Path(page_name): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    ContentPageDao::get_page_by_name(&state.pool, &page_name)
        .await?
        .ok_or_else(|| anyhow!("Can't attach to a non existent page"))?;

    while let Some(field) = multipart.next_field().await? {
        let name = field
            .file_name()
            .ok_or_else(|| anyhow!("Files need names!"))?
            .to_string();
        let data = field.bytes().await?;

        let attachment = AttachmentDao {
            parent_page_name: page_name.clone(),
            attachment_name: name.to_string(),
            mime_type: mime_guess::from_path(&name)
                .first()
                .ok_or_else(|| anyhow!("Unable to figure out mime type {}", name))?
                .to_string(),
            attachment_content: data.to_vec(),
        };

        attachment.save(&state.pool).await?;
    }

    let mut headers = HeaderMap::new();
    headers.insert("HX-Refresh", "true".parse()?);

    Ok(headers)
}

pub async fn delete_attachment(
    State(state): State<AppState>,
    session_data: SessionData,
    Path((page_name, attachment_name)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    if let AuthenticationState::Authenticated(user) = session_data.auth_state {
        if user.role != Role::Admin {
            return Err(anyhow!("Invalid Permission").into());
        }
    }

    let attachment =
        AttachmentDao::find_attachment(&state.pool, &page_name, &attachment_name).await?;

    if let Some(a) = attachment {
        a.delete(&state.pool).await?;

        let mut headers = HeaderMap::new();
        headers.insert("HX-Refresh", "true".parse()?);

        Ok(headers)
    } else {
        Err(anyhow!("Attachment does not exist").into())
    }
}
