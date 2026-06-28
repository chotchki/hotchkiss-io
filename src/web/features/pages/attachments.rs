use std::io::Cursor;

use crate::{
    db::dao::attachments::AttachmentDao,
    web::{app_error::AppError, app_state::AppState},
};
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use http::{header, StatusCode};
use image::{ImageFormat, ImageReader};
use serde::Deserialize;

const MAX_WIDTH: u32 = 8000; //No reason to resize above this width

/// Serve-only fallback for legacy `/attachments` byte URLs (BZ.8 Stage 2). The
/// upload / list / delete WRITE paths were retired — media (`/admin/media` + the
/// inline editor upload) is the management UI now, and the migration rewrote
/// content refs to `/media`. These GET routes stay until Stage 3 drops the
/// `attachments` table, covering any not-yet-rewritten or externally-linked URL.
pub fn attachments_router() -> Router<AppState> {
    Router::new()
        .route("/id/{:attachment_id}", get(load_attachment_by_id))
        .route("/{:page_id}/{:attachment_name}", get(load_attachment))
}

#[derive(Debug, Deserialize)]
pub struct AttachmentSizeParams {
    pub width: Option<u32>,
}

fn render_attachment(name: String, mime: String, buffer: Vec<u8>) -> Result<Response, AppError> {
    let headers = [
        (header::CONTENT_TYPE, mime.to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        ),
        (header::CONTENT_LENGTH, buffer.len().to_string()),
    ];

    Ok((headers, buffer).into_response())
}

/// Height that preserves aspect ratio when scaling an `orig_w`×`orig_h` image to
/// `target_w`. Multiplies before dividing (the old `orig_h/orig_w*target_w`
/// truncated to 0 for any landscape image → a 0-height/blank thumbnail), widens
/// to u64 to avoid overflow on large images, and floors at 1 so no dimension is
/// ever 0.
fn scaled_height(orig_w: u32, orig_h: u32, target_w: u32) -> u32 {
    ((u64::from(target_w) * u64::from(orig_h)) / u64::from(orig_w.max(1))).max(1) as u32
}

fn maybe_resize_attachment(
    attachment: Option<AttachmentDao>,
    size_params: AttachmentSizeParams,
) -> Result<Response, AppError> {
    let Some(a) = attachment else {
        return Ok((StatusCode::NOT_FOUND, "Attachment does not exist").into_response());
    };

    if a.attachment_name.ends_with(".stl") {
        render_attachment(a.attachment_name, a.mime_type, a.attachment_content)
    } else if let Some(width) = size_params.width {
        if width > MAX_WIDTH {
            return Ok((StatusCode::BAD_REQUEST, "Size too large").into_response());
        }

        let image = ImageReader::new(Cursor::new(a.attachment_content))
            .with_guessed_format()?
            .decode()?;

        let nheight = scaled_height(image.width(), image.height(), width);

        let new_image = image.resize(width, nheight, image::imageops::FilterType::Gaussian);

        let mut new_image_buf: Vec<u8> = Vec::new();
        new_image.write_to(
            &mut Cursor::new(&mut new_image_buf),
            image::ImageFormat::Avif,
        )?;

        render_attachment(
            a.attachment_name,
            ImageFormat::Avif.to_mime_type().to_string(),
            new_image_buf,
        )
    } else {
        render_attachment(a.attachment_name, a.mime_type, a.attachment_content)
    }
}

pub async fn load_attachment(
    State(state): State<AppState>,
    Path((page_id, attachment_name)): Path<(i64, String)>,
    Query(size_params): Query<AttachmentSizeParams>,
) -> Result<Response, AppError> {
    let attachment =
        AttachmentDao::find_attachment_by_name(&state.pool, page_id, &attachment_name).await?;

    maybe_resize_attachment(attachment, size_params)
}

pub async fn load_attachment_by_id(
    State(state): State<AppState>,
    Path(attachment_id): Path<i64>,
    Query(size_params): Query<AttachmentSizeParams>,
) -> Result<Response, AppError> {
    let attachment = AttachmentDao::find_attachment_by_id(&state.pool, attachment_id).await?;

    maybe_resize_attachment(attachment, size_params)
}

#[cfg(test)]
mod tests {
    use super::scaled_height;

    #[test]
    fn scaled_height_preserves_aspect_and_never_zero() {
        // Landscape: the old integer `h/w*target` gave 0; aspect-correct is 50.
        assert_eq!(scaled_height(1000, 500, 100), 50);
        // Portrait scales up proportionally.
        assert_eq!(scaled_height(500, 1000, 100), 200);
        // A very wide, thin image floors at 1 rather than collapsing to 0.
        assert_eq!(scaled_height(10000, 1, 100), 1);
        // Square.
        assert_eq!(scaled_height(800, 800, 400), 400);
        // Degenerate orig width doesn't panic (divide-by-zero guard).
        assert!(scaled_height(0, 500, 100) >= 1);
    }
}
