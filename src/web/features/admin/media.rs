//! Admin media library (Phase BZ) — under the `/admin` nest, so `require_admin`
//! gates the GET and the global mutation layer gates POST/DELETE.
//!
//! Upload ingest: each dropped file is stored content-addressed, ffprobe'd for
//! its typed facts (kind / mime / codecs / dims / duration — never trusting the
//! filename), and recorded as a `media_variant`. All files in ONE upload group
//! into ONE `media` item; a video also gets an auto-poster (ffmpeg frame-grab →
//! AVIF, stored as an image variant). After the fact, "+ add encode" appends a
//! variant to an EXISTING item — drop the other codec, or an image to set the
//! poster.

use anyhow::{anyhow, Result};
use askama::Template;
use axum::extract::{Multipart, Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use http::StatusCode;
use serde_json::json;
use sqlx::SqlitePool;

use crate::db::dao::crypto_key::CryptoKey;
use crate::db::dao::media::{MediaDao, MediaKind, MediaVariantDao};
use crate::media::poster::generate_poster;
use crate::media::probe::{probe, Probed};
use crate::media::{media_url_key, MediaStore};
use crate::web::authentication_state::AuthenticationState;
use crate::web::features::media::render_embed_html;
use crate::web::features::top_bar::TopBar;
use crate::web::htmx_responses::htmx_refresh;
use crate::web::{app_error::AppError, app_state::AppState, html_template::HtmlTemplate, session::SessionData};

/// CryptoKey row id for the media-URL HMAC secret (session signing key is id 1).
const MEDIA_HMAC_KEY_ID: i64 = 2;

#[derive(Template)]
#[template(path = "admin/media.html")]
pub struct MediaLibraryTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub cards: Vec<MediaCard>,
}

pub struct MediaCard {
    pub media_id: i64,
    pub media_ref: String,
    pub title: String,
    pub kind: String,
    /// Image kind: the image's url_key, shown as a thumbnail.
    pub thumb_url_key: Option<String>,
    /// Video kind: the playable `<video>` element (reuses the embed render).
    pub play_html: Option<String>,
    pub variants: Vec<VariantRow>,
    /// Lowercased "ref title" for the client-side name filter.
    pub search: String,
}

pub struct VariantRow {
    pub variant_id: i64,
    pub label: String,
    pub size: String,
}

pub async fn show_media_library(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let media = MediaDao::find_all(&state.pool).await?;
    let mut cards = Vec::with_capacity(media.len());
    for m in media {
        let variants = MediaVariantDao::find_by_media_id(&state.pool, m.media_id).await?;
        // Video → a playable <video> (poster + sources, preload metadata). Other
        // kinds → an image-variant thumbnail (image itself, or none → kind icon).
        let (thumb_url_key, play_html) = if m.kind == "video" {
            (None, Some(render_embed_html(&m, &variants)))
        } else {
            let thumb = variants
                .iter()
                .rev()
                .find(|v| v.mime.starts_with("image/"))
                .map(|v| v.url_key.clone());
            (thumb, None)
        };
        let variant_rows = variants
            .iter()
            .map(|v| VariantRow {
                variant_id: v.variant_id,
                label: v.codecs.clone().unwrap_or_else(|| v.mime.clone()),
                size: format_bytes(v.bytes),
            })
            .collect();
        let title = m.title.clone().unwrap_or_else(|| m.media_ref.clone());
        let search = title.to_lowercase();
        cards.push(MediaCard {
            media_id: m.media_id,
            media_ref: m.media_ref,
            title,
            kind: m.kind,
            thumb_url_key,
            play_html,
            variants: variant_rows,
            search,
        });
    }
    Ok(HtmlTemplate(MediaLibraryTemplate {
        top_bar: TopBar::create(&state.pool, "admin").await?,
        auth_state: session_data.auth_state,
        cards,
    })
    .into_response())
}

/// Human-readable byte size for the per-stream display.
fn format_bytes(b: i64) -> String {
    let bf = b as f64;
    if bf >= 1_048_576.0 {
        format!("{:.1} MB", bf / 1_048_576.0)
    } else if bf >= 1024.0 {
        format!("{:.0} KB", bf / 1024.0)
    } else {
        format!("{b} B")
    }
}

pub async fn upload_media(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut ref_input: Option<String> = None;
    let mut title: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow!("reading multipart: {e}"))?
    {
        let name = field.name().unwrap_or("").to_string();
        if let Some(fname) = field.file_name().map(|s| s.to_string()) {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| anyhow!("reading uploaded file: {e}"))?;
            if !bytes.is_empty() {
                files.push((fname, bytes.to_vec()));
            }
        } else {
            let value = field.text().await.unwrap_or_default();
            match name.as_str() {
                "media_ref" if !value.trim().is_empty() => ref_input = Some(value),
                "title" if !value.trim().is_empty() => title = Some(value),
                _ => {}
            }
        }
    }

    if files.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "No files in the upload").into_response());
    }

    // The URL ref is an OPAQUE, unguessable token (NOT a slug) — the byte route is
    // already HMAC'd; this closes the slug-guess gap for unpublished media. The
    // human name lives in `title` (library display / search / rename), derived
    // from the filename when not given.
    let media_ref = uuid::Uuid::now_v7().simple().to_string();
    let title = title
        .or(ref_input)
        .or_else(|| Some(strip_media_suffixes(&files[0].0)))
        .filter(|s| !s.trim().is_empty());

    let hmac_key = CryptoKey::get_or_create(&state.pool, MEDIA_HMAC_KEY_ID)
        .await?
        .key_value;

    // Store + probe every file (off the runtime).
    let mut ingested: Vec<(String, Probed, i64)> = Vec::with_capacity(files.len());
    for (fname, bytes) in files {
        ingested.push(store_and_probe(state.media_store.clone(), fname, bytes).await?);
    }

    // The media row takes its kind + dims/duration from the first file.
    let first = &ingested[0].1;
    let kind = first.kind;
    let media = MediaDao::create(
        &state.pool,
        media_ref.clone(),
        kind,
        title,
        first.width,
        first.height,
        first.duration_ms,
    )
    .await?;

    for (sha, probed, len) in &ingested {
        create_variant(
            &state.pool,
            &hmac_key,
            media.media_id,
            sha.clone(),
            probed.mime.clone(),
            probed.codecs.clone(),
            *len,
        )
        .await?;
    }

    // Auto-poster for video (non-fatal — the video still plays without one).
    if kind == MediaKind::Video {
        maybe_add_poster(&state, &hmac_key, media.media_id, ingested[0].0.clone()).await;
    }

    Ok(Json(json!({
        "media_id": media.media_id,
        "media_ref": media_ref,
        "markdown": format!("![](/media/{media_ref})"),
    }))
    .into_response())
}

/// Append a variant (another encode, or an image → poster) to an existing item.
pub async fn add_encode(
    State(state): State<AppState>,
    Path(media_id): Path<i64>,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let hmac_key = CryptoKey::get_or_create(&state.pool, MEDIA_HMAC_KEY_ID)
        .await?
        .key_value;
    let existing = MediaVariantDao::find_by_media_id(&state.pool, media_id).await?;

    let mut saw_file = false;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow!("reading multipart: {e}"))?
    {
        if let Some(fname) = field.file_name().map(|s| s.to_string()) {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| anyhow!("reading uploaded file: {e}"))?;
            if bytes.is_empty() {
                continue;
            }
            saw_file = true;
            let (sha, probed, len) =
                store_and_probe(state.media_store.clone(), fname, bytes.to_vec()).await?;
            // Dedup: the same bytes are already an encode of this item → no-op.
            if existing.iter().any(|v| v.sha256 == sha) {
                continue;
            }
            create_variant(&state.pool, &hmac_key, media_id, sha, probed.mime, probed.codecs, len)
                .await?;
        }
    }

    if !saw_file {
        return Ok((StatusCode::BAD_REQUEST, "No file in the upload").into_response());
    }
    // A file present but fully deduped is an idempotent no-op — still refresh.
    Ok(htmx_refresh())
}

pub async fn delete_media(
    State(state): State<AppState>,
    Path(media_id): Path<i64>,
) -> Result<Response, AppError> {
    MediaDao::delete_by_id(&state.pool, media_id).await?;
    Ok(htmx_refresh())
}

/// Delete one stored encoding (per-stream). Leaves the rest of the item intact.
pub async fn delete_variant(
    State(state): State<AppState>,
    Path(variant_id): Path<i64>,
) -> Result<Response, AppError> {
    MediaVariantDao::delete_by_id(&state.pool, variant_id).await?;
    Ok(htmx_refresh())
}

#[derive(serde::Deserialize)]
pub struct RenameForm {
    pub title: String,
}

/// Rename the display title (the `media_ref` stays fixed — see DAO note).
pub async fn rename_media(
    State(state): State<AppState>,
    Path(media_id): Path<i64>,
    axum::Form(form): axum::Form<RenameForm>,
) -> Result<Response, AppError> {
    MediaDao::update_title(&state.pool, media_id, &form.title).await?;
    Ok(htmx_refresh())
}

/// Store bytes content-addressed + ffprobe the stored file, off the async runtime.
async fn store_and_probe(
    store: MediaStore,
    filename: String,
    bytes: Vec<u8>,
) -> Result<(String, Probed, i64)> {
    let len = bytes.len() as i64;
    let (sha, probed) = tokio::task::spawn_blocking(move || -> Result<_> {
        let sha = store.store(&bytes)?;
        let probed = probe(&store.path_for(&sha), &filename)?;
        Ok((sha, probed))
    })
    .await
    .map_err(|e| anyhow!("ingest task panicked: {e}"))??;
    Ok((sha, probed, len))
}

async fn create_variant(
    pool: &SqlitePool,
    hmac_key: &[u8],
    media_id: i64,
    sha: String,
    mime: String,
    codecs: Option<String>,
    bytes: i64,
) -> Result<()> {
    let url_key = media_url_key(hmac_key, &sha)?;
    MediaVariantDao::create(pool, media_id, sha, url_key, mime, codecs, bytes).await?;
    Ok(())
}

/// Frame-grab a poster for a video and add it as an image variant. Best-effort:
/// a failure is logged, not surfaced — the video plays fine without a poster.
async fn maybe_add_poster(state: &AppState, hmac_key: &[u8], media_id: i64, video_sha: String) {
    let store = state.media_store.clone();
    let result: Result<(String, i64)> = async {
        let path_store = store.clone();
        let avif = tokio::task::spawn_blocking(move || generate_poster(&path_store.path_for(&video_sha)))
            .await
            .map_err(|e| anyhow!("poster task panicked: {e}"))??;
        let len = avif.len() as i64;
        let sha = tokio::task::spawn_blocking(move || store.store(&avif))
            .await
            .map_err(|e| anyhow!("poster store task panicked: {e}"))??;
        Ok((sha, len))
    }
    .await;

    match result {
        Ok((sha, len)) => {
            if let Err(e) =
                create_variant(&state.pool, hmac_key, media_id, sha, "image/avif".to_string(), None, len)
                    .await
            {
                tracing::warn!("auto-poster variant insert failed (media {media_id}): {e:?}");
            }
        }
        Err(e) => tracing::warn!("auto-poster generation failed (media {media_id}): {e:?}"),
    }
}

/// `intro.av1.mp4` / `intro.mp4` → `intro` — drop the extension, then a trailing
/// codec tag, so a video's encodes derive the same base ref.
fn strip_media_suffixes(filename: &str) -> String {
    let stem = filename.rsplit_once('.').map(|(s, _)| s).unwrap_or(filename);
    let lower = stem.to_ascii_lowercase();
    for tag in [".av1", ".hevc", ".hvc1", ".h264", ".vp9", ".webm"] {
        if lower.ends_with(tag) {
            return stem[..stem.len() - tag.len()].to_string();
        }
    }
    stem.to_string()
}

