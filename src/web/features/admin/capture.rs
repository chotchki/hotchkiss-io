//! EB — mobile quick-capture. Snap a photo on the phone → a draft blog post (or
//! an append onto a recent one) with the embed already in place. The GET renders
//! the camera-first page; the POST is the PAGE WRITE only — bytes travel the
//! canonical `POST /media` lane (capture.js drives it), so this handler sees a
//! `media_ref`, never a file. Both modes run through the PageWrite service (the
//! ONE policy path); a draft is the CU far-future sentinel, so it publishes
//! later with the ordinary Publish-now button. An orphaned media item (upload
//! succeeded, this POST failed) is acceptable — the admin library lists it.

use crate::{
    db::dao::{content_pages::ContentPageDao, media::MediaDao},
    web::{
        app_error::AppError,
        app_state::AppState,
        authentication_state::AuthenticationState,
        features::pages::write::{self, PageUpdate, PageWriteError},
        features::top_bar::TopBar,
        html_template::HtmlTemplate,
        responder::{ClientKind, WriteOutcome},
        session::SessionData,
    },
};
use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::types::chrono::Utc;

/// The CU draft sentinel in `datetime-local` form — `parse_local_datetime` reads
/// it inside `update_page`, mirroring the Unpublish button's stored value.
const DRAFT_SENTINEL_LOCAL: &str = "2999-01-01T00:00:00";

/// How many recent blog posts the append picker offers.
const RECENT_TARGETS: i64 = 12;

#[derive(Template)]
#[template(path = "admin/capture.html")]
pub struct CaptureTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub recent: Vec<RecentTarget>,
}

pub struct RecentTarget {
    pub slug: String,
    pub title: String,
    pub scheduled: bool,
}

pub async fn show_capture(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let recent = recent_targets(&state).await?;
    let template = CaptureTemplate {
        top_bar: TopBar::create(&state.pool, "admin", session_data.auth_state.role()).await?,
        auth_state: session_data.auth_state,
        recent,
    };
    Ok(HtmlTemplate(template).into_response())
}

async fn recent_targets(state: &AppState) -> Result<Vec<RecentTarget>, AppError> {
    let Some(blog) = ContentPageDao::find_by_name(&state.pool, None, "blog").await? else {
        return Ok(Vec::new());
    };
    let children = ContentPageDao::find_by_parent_newest_first(
        &state.pool,
        Some(blog.page_id),
        Some(RECENT_TARGETS),
    )
    .await?;
    Ok(children
        .iter()
        .map(|p| RecentTarget {
            slug: p.page_name.clone(),
            title: p.display_title(),
            scheduled: p.is_scheduled(),
        })
        .collect())
}

#[derive(Deserialize)]
pub struct CaptureForm {
    pub media_ref: String,
    /// `draft` (new scheduled-draft blog post) or `append` (onto `target`).
    pub mode: String,
    /// Blog-child slug for `append` — the picker constrains targets to blog
    /// posts, and the server resolves ONLY under `/blog` (no arbitrary paths).
    #[serde(default)]
    pub target: String,
    /// Optional markdown caption, emitted as a paragraph above the embed.
    #[serde(default)]
    pub caption: String,
}

pub async fn capture_post(
    State(state): State<AppState>,
    client: ClientKind,
    Form(form): Form<CaptureForm>,
) -> Result<Response, AppError> {
    // The ref must be an existing media item — a typo'd/foreign ref is the
    // caller's bug, not a 500.
    let Some(media) = MediaDao::find_by_ref(&state.pool, form.media_ref.trim()).await? else {
        return Ok((StatusCode::BAD_REQUEST, "unknown media_ref").into_response());
    };

    let mut body = String::new();
    let caption = form.caption.trim();
    if !caption.is_empty() {
        body.push_str(caption);
        body.push_str("\n\n");
    }
    body.push_str(&format!("![](/media/{})", media.media_ref));

    let written = match form.mode.as_str() {
        "draft" => {
            // Second-precision title so rapid-fire captures can't slug-collide
            // (and after the first shot the client auto-switches to append).
            let title = format!("Capture {}", Utc::now().format("%Y-%m-%d %H:%M:%S"));
            let w = match write::create_page(&state.pool, &["blog"], &title).await {
                Ok(w) => w,
                Err(PageWriteError::DuplicateSlug { slug, .. }) => {
                    return Ok((
                        StatusCode::CONFLICT,
                        format!("a capture draft '{slug}' already exists — try again"),
                    )
                        .into_response());
                }
                Err(PageWriteError::NotFound) => {
                    return Ok((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "the blog section is missing",
                    )
                        .into_response());
                }
                Err(PageWriteError::EmptyTitle) => unreachable!("generated title always slugs"),
                Err(PageWriteError::Internal(e)) => return Err(e.into()),
            };
            write::update_page(
                &state.pool,
                &state.site_host,
                &["blog", &w.slug],
                PageUpdate {
                    title: Some(title),
                    markdown: body,
                    creation_date: Some(DRAFT_SENTINEL_LOCAL.to_string()),
                    // The photo IS the card/cover for a photo post.
                    cover_ref: Some(media.media_ref.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(page_write_internal)?
        }
        "append" => {
            let target = form.target.trim();
            if target.is_empty() {
                return Ok((StatusCode::BAD_REQUEST, "append needs a target").into_response());
            }
            let pages = ContentPageDao::find_by_path(&state.pool, &["blog", target]).await?;
            let Some(existing) = pages.last() else {
                return Ok((StatusCode::NOT_FOUND, "no such blog post").into_response());
            };
            // update_page is FULL-replace (absent title clears, absent cover
            // CLEARS — the editor asymmetry), so read-modify-write and carry
            // everything. Cover: keep the existing one, else this photo.
            let existing_cover =
                crate::web::features::media::cover_media_id_for(&state.pool, existing.page_id)
                    .await;
            let cover_ref = match existing_cover {
                Some(id) => MediaDao::find_by_id(&state.pool, id)
                    .await?
                    .map(|m| m.media_ref),
                None => None,
            }
            .unwrap_or_else(|| media.media_ref.clone());
            let mut markdown = existing.page_markdown.trim_end().to_string();
            if !markdown.is_empty() {
                markdown.push_str("\n\n");
            }
            markdown.push_str(&body);
            markdown.push('\n');
            write::update_page(
                &state.pool,
                &state.site_host,
                &["blog", target],
                PageUpdate {
                    title: existing.page_title.clone(),
                    category: existing.page_category.clone(),
                    markdown,
                    order: existing.page_order,
                    creation_date: None,
                    min_role: None,
                    cover_ref: Some(cover_ref),
                },
            )
            .await
            .map_err(page_write_internal)?
        }
        _ => return Ok((StatusCode::BAD_REQUEST, "mode must be draft or append").into_response()),
    };

    // JSON (capture.js) reads the envelope and STAYS on the capture page; a
    // no-JS form lands in the draft's editor via the native 303.
    let url = format!("{}?edit", written.pages_url());
    Ok(WriteOutcome::navigate(url, Some(written)).into_response(client))
}

/// After the earlier typed arms, anything left is Internal (update_page on a
/// path we JUST resolved can't be NotFound in practice; fold it anyway).
fn page_write_internal(e: PageWriteError) -> AppError {
    match e {
        PageWriteError::Internal(e) => e.into(),
        other => anyhow::anyhow!("unexpected page-write failure: {other:?}").into(),
    }
}
