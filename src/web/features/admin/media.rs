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
use crate::media::resize::{responsive_avif_variants, ResizedImage};
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
    pub storage: Vec<StorageRow>,
}

/// A row of the storage panel — a configured media root + its free/total space
/// (humanized) and role. `free`/`total` are `None` when the root is unavailable
/// (missing or unmounted).
pub struct StorageRow {
    pub path: String,
    pub free: Option<String>,
    pub total: Option<String>,
    pub is_write_target: bool,
    pub below_margin: bool,
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
    /// First variant's HMAC `url_key` → the absolute `/media/file/<key>` direct
    /// link, for "Copy link" (a private, unguessable share/download URL).
    pub share_url_key: Option<String>,
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
        // First variant's url_key → the absolute /media/file/<key> share link.
        let share_url_key = variants.first().map(|v| v.url_key.clone());
        cards.push(MediaCard {
            media_id: m.media_id,
            media_ref: m.media_ref,
            title,
            kind: m.kind,
            thumb_url_key,
            play_html,
            variants: variant_rows,
            search,
            share_url_key,
        });
    }
    // Storage panel — each configured root + its free space, so multi-drive
    // placement isn't silent (which one's being written to, which are full/offline).
    let storage = state
        .media_store
        .roots_status()
        .into_iter()
        .map(|s| StorageRow {
            path: s.path.to_string_lossy().into_owned(),
            free: s.free_bytes.map(|b| format_bytes(b as i64)),
            total: s.total_bytes.map(|b| format_bytes(b as i64)),
            is_write_target: s.is_write_target,
            below_margin: s.below_margin,
        })
        .collect();

    Ok(HtmlTemplate(MediaLibraryTemplate {
        top_bar: TopBar::create(&state.pool, "admin").await?,
        auth_state: session_data.auth_state,
        cards,
        storage,
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
    let mut ingested: Vec<(String, Probed, i64, String)> = Vec::new();
    let mut ref_input: Option<String> = None;
    let mut title: Option<String> = None;
    let mut first_filename: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow!("reading multipart: {e}"))?
    {
        if let Some(fname) = field.file_name().map(|s| s.to_string()) {
            // Stream the file straight to the content store (O(chunk) memory) —
            // hashed + written as it arrives, NEVER buffered whole — then ffprobe
            // the stored file. This is what lets a multi-GB upload work without
            // OOMing the process (the old path slurped the field into a Vec<u8>).
            let mut staged = state.media_store.stage().await?;
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|e| anyhow!("reading uploaded file: {e}"))?
            {
                staged.write_chunk(&chunk).await?;
            }
            if staged.is_empty() {
                continue; // empty part → drop it (the staged temp self-cleans)
            }
            let (sha, len, root) = staged.commit(&state.media_store).await?;
            let root = root.to_string_lossy().into_owned();
            let probed = probe_stored(
                state.media_store.clone(),
                sha.clone(),
                fname.clone(),
                Some(root.clone()),
            )
            .await?;
            if first_filename.is_none() {
                first_filename = Some(fname);
            }
            ingested.push((sha, probed, len as i64, root));
        } else {
            let name = field.name().unwrap_or("").to_string();
            let value = field.text().await.unwrap_or_default();
            match name.as_str() {
                "media_ref" if !value.trim().is_empty() => ref_input = Some(value),
                "title" if !value.trim().is_empty() => title = Some(value),
                _ => {}
            }
        }
    }

    if ingested.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "No files in the upload").into_response());
    }

    // The URL ref is an OPAQUE, unguessable token (NOT a slug) — the byte route is
    // already HMAC'd; this closes the slug-guess gap for unpublished media. The
    // human name lives in `title` (library display / search / rename), derived
    // from the filename when not given.
    let media_ref = uuid::Uuid::now_v7().simple().to_string();
    let title = title
        .or(ref_input)
        .or_else(|| first_filename.as_deref().map(strip_media_suffixes))
        .filter(|s| !s.trim().is_empty());

    let hmac_key = CryptoKey::get_or_create(&state.pool, MEDIA_HMAC_KEY_ID)
        .await?
        .key_value;

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

    for (sha, probed, len, root) in &ingested {
        create_variant(
            &state.pool,
            &hmac_key,
            media.media_id,
            sha.clone(),
            probed.mime.clone(),
            probed.codecs.clone(),
            *len,
            Some(root.clone()),
            probed.width,
            probed.height,
        )
        .await?;
    }

    // Responsive image variants (Phase CN): for an image, generate width-stepped
    // AVIFs from the original so the render can emit a srcset and the browser
    // pulls an appropriately-sized file. Best-effort — the original still serves.
    if kind == MediaKind::Image {
        if let (Some(w), sha) = (first.width, ingested[0].0.clone()) {
            maybe_add_responsive_variants(&state, &hmac_key, media.media_id, sha, w).await;
        }
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
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow!("reading multipart: {e}"))?
    {
        if let Some(fname) = field.file_name().map(|s| s.to_string()) {
            let mut staged = state.media_store.stage().await?;
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|e| anyhow!("reading uploaded file: {e}"))?
            {
                staged.write_chunk(&chunk).await?;
            }
            if staged.is_empty() {
                continue;
            }
            saw_file = true;
            let (sha, len, root) = staged.commit(&state.media_store).await?;
            // Dedup: the same bytes are already an encode of this item → no-op
            // (commit already deduped the blob on disk; skip the metadata row).
            if existing.iter().any(|v| v.sha256 == sha) {
                continue;
            }
            let root = root.to_string_lossy().into_owned();
            let probed = probe_stored(
                state.media_store.clone(),
                sha.clone(),
                fname,
                Some(root.clone()),
            )
            .await?;
            create_variant(
                &state.pool,
                &hmac_key,
                media_id,
                sha,
                probed.mime,
                probed.codecs,
                len as i64,
                Some(root),
                probed.width,
                probed.height,
            )
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

/// ffprobe an ALREADY-stored file (its bytes are on disk — storing now happens via
/// streaming, [`MediaStore::stage`]). Resolves the path across the media roots
/// (the `hint` is the just-written root → O(1)), off the async runtime.
async fn probe_stored(
    store: MediaStore,
    sha: String,
    filename: String,
    hint: Option<String>,
) -> Result<Probed> {
    tokio::task::spawn_blocking(move || {
        let path = store
            .resolve_path(&sha, hint.as_deref())
            .ok_or_else(|| anyhow!("just-stored media {sha} not found in any media root"))?;
        probe(&path, &filename)
    })
    .await
    .map_err(|e| anyhow!("probe task panicked: {e}"))?
}

#[allow(clippy::too_many_arguments)]
async fn create_variant(
    pool: &SqlitePool,
    hmac_key: &[u8],
    media_id: i64,
    sha: String,
    mime: String,
    codecs: Option<String>,
    bytes: i64,
    storage_root: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
) -> Result<()> {
    let url_key = media_url_key(hmac_key, &sha)?;
    MediaVariantDao::create(
        pool,
        media_id,
        sha,
        url_key,
        mime,
        codecs,
        bytes,
        storage_root,
        width,
        height,
    )
    .await?;
    Ok(())
}

/// Frame-grab a poster for a video and add it as an image variant. Best-effort:
/// a failure is logged, not surfaced — the video plays fine without a poster.
async fn maybe_add_poster(state: &AppState, hmac_key: &[u8], media_id: i64, video_sha: String) {
    let store = state.media_store.clone();
    let result: Result<(String, i64, std::path::PathBuf)> = async {
        let path_store = store.clone();
        let avif = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let path = path_store
                .resolve_path(&video_sha, None)
                .ok_or_else(|| anyhow!("poster source {video_sha} not found in any media root"))?;
            generate_poster(&path)
        })
        .await
        .map_err(|e| anyhow!("poster task panicked: {e}"))??;
        let len = avif.len() as i64;
        let (sha, root) = tokio::task::spawn_blocking(move || store.store(&avif))
            .await
            .map_err(|e| anyhow!("poster store task panicked: {e}"))??;
        Ok((sha, len, root))
    }
    .await;

    match result {
        Ok((sha, len, root)) => {
            if let Err(e) = create_variant(
                &state.pool,
                hmac_key,
                media_id,
                sha,
                "image/avif".to_string(),
                None,
                len,
                Some(root.to_string_lossy().into_owned()),
                None,
                None,
            )
            .await
            {
                tracing::warn!("auto-poster variant insert failed (media {media_id}): {e:?}");
            }
        }
        Err(e) => tracing::warn!("auto-poster generation failed (media {media_id}): {e:?}"),
    }
}

/// Generate width-stepped AVIF variants for an image so the render can emit a
/// `srcset` (Phase CN). Best-effort: a failure is logged, not surfaced — the
/// original variant still serves. Each resized blob is content-addressed (dedup'd
/// like any other) and recorded as an `image/avif` variant carrying its width.
async fn maybe_add_responsive_variants(
    state: &AppState,
    hmac_key: &[u8],
    media_id: i64,
    original_sha: String,
    source_width: i64,
) {
    let store = state.media_store.clone();
    let result: Result<()> = async {
        let path_store = store.clone();
        let resized = tokio::task::spawn_blocking(move || -> Result<Vec<ResizedImage>> {
            let path = path_store
                .resolve_path(&original_sha, None)
                .ok_or_else(|| anyhow!("resize source {original_sha} not found in any media root"))?;
            responsive_avif_variants(&path, source_width.max(0) as u32)
        })
        .await
        .map_err(|e| anyhow!("resize task panicked: {e}"))??;

        for r in resized {
            let store = store.clone();
            let bytes = r.avif;
            let len = bytes.len() as i64;
            let (sha, root) = tokio::task::spawn_blocking(move || store.store(&bytes))
                .await
                .map_err(|e| anyhow!("resize store task panicked: {e}"))??;
            create_variant(
                &state.pool,
                hmac_key,
                media_id,
                sha,
                "image/avif".to_string(),
                None,
                len,
                Some(root.to_string_lossy().into_owned()),
                Some(r.width as i64),
                Some(r.height as i64),
            )
            .await?;
        }
        Ok(())
    }
    .await;

    if let Err(e) = result {
        tracing::warn!("responsive image variants failed (media {media_id}): {e:?}");
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

