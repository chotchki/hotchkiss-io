//! Admin media library (Phase BZ) — under the `/admin` nest, so `require_admin`
//! gates the GET and the global mutation layer gates POST/DELETE.
//!
//! Upload ingest: each dropped file is stored content-addressed, ffprobe'd for
//! its typed facts (kind / mime / codecs / dims / duration — never trusting the
//! filename), and recorded as a `media_variant`. All files in ONE upload group
//! into ONE `media` item (so a video's AV1 + HEVC encodes share a `media_ref`).
//! The response is JSON `{media_ref}` so the same endpoint serves the library
//! drag-drop AND the in-editor insert.

use anyhow::{anyhow, Result};
use askama::Template;
use axum::extract::{Multipart, Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use http::StatusCode;
use serde_json::json;
use sqlx::SqlitePool;

use crate::db::dao::crypto_key::CryptoKey;
use crate::db::dao::media::{MediaDao, MediaVariantDao};
use crate::media::{media_url_key, probe::probe};
use crate::web::authentication_state::AuthenticationState;
use crate::web::features::top_bar::TopBar;
use crate::web::htmx_responses::htmx_refresh;
use crate::web::util::slug::slugify;
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
    /// For image kind, the variant's url_key (so the card shows the image); else None.
    pub thumb_url_key: Option<String>,
    pub codecs: Vec<String>,
    pub variant_count: usize,
}

pub async fn show_media_library(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let media = MediaDao::find_all(&state.pool).await?;
    let mut cards = Vec::with_capacity(media.len());
    for m in media {
        let variants = MediaVariantDao::find_by_media_id(&state.pool, m.media_id).await?;
        let thumb_url_key = if m.kind == "image" {
            variants.first().map(|v| v.url_key.clone())
        } else {
            None
        };
        let codecs = variants.iter().filter_map(|v| v.codecs.clone()).collect();
        cards.push(MediaCard {
            media_id: m.media_id,
            title: m.title.clone().unwrap_or_else(|| m.media_ref.clone()),
            media_ref: m.media_ref,
            kind: m.kind,
            thumb_url_key,
            codecs,
            variant_count: variants.len(),
        });
    }
    Ok(HtmlTemplate(MediaLibraryTemplate {
        top_bar: TopBar::create(&state.pool, "admin").await?,
        auth_state: session_data.auth_state,
        cards,
    })
    .into_response())
}

/// The stored + probed facts for one uploaded file.
struct Ingested {
    sha: String,
    bytes: i64,
    probed: crate::media::probe::Probed,
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

    // Derive a unique ref: the given slug, else the first filename minus its
    // codec/extension suffix (so intro.av1.mp4 + intro.mp4 both → "intro").
    let base = ref_input.unwrap_or_else(|| strip_media_suffixes(&files[0].0));
    let mut slug = slugify(&base);
    if slug.is_empty() {
        slug = "media".to_string();
    }
    let media_ref = unique_ref(&state.pool, slug).await?;

    let hmac_key = CryptoKey::get_or_create(&state.pool, MEDIA_HMAC_KEY_ID)
        .await?
        .key_value;

    // Store + probe each file OFF the async runtime (disk write + ffprobe block).
    let mut ingested: Vec<Ingested> = Vec::with_capacity(files.len());
    for (fname, bytes) in files {
        let store = state.media_store.clone();
        let len = bytes.len() as i64;
        let (sha, probed) = tokio::task::spawn_blocking(move || -> Result<_> {
            let sha = store.store(&bytes)?;
            let probed = probe(&store.path_for(&sha), &fname)?;
            Ok((sha, probed))
        })
        .await
        .map_err(|e| anyhow!("ingest task panicked: {e}"))??;
        ingested.push(Ingested {
            sha,
            bytes: len,
            probed,
        });
    }

    // The media row takes its kind + dims/duration from the first file (a video's
    // encodes agree; an image upload is a single file).
    let first = &ingested[0].probed;
    let media = MediaDao::create(
        &state.pool,
        media_ref.clone(),
        first.kind,
        title,
        first.width,
        first.height,
        first.duration_ms,
    )
    .await?;

    for i in &ingested {
        let url_key = media_url_key(&hmac_key, &i.sha)?;
        MediaVariantDao::create(
            &state.pool,
            media.media_id,
            i.sha.clone(),
            url_key,
            i.probed.mime.clone(),
            i.probed.codecs.clone(),
            i.bytes,
        )
        .await?;
    }

    Ok(Json(json!({
        "media_ref": media_ref,
        "markdown": format!("![](/media/{media_ref})"),
    }))
    .into_response())
}

pub async fn delete_media(
    State(state): State<AppState>,
    Path(media_id): Path<i64>,
) -> Result<Response, AppError> {
    MediaDao::delete_by_id(&state.pool, media_id).await?;
    Ok(htmx_refresh())
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

async fn unique_ref(pool: &SqlitePool, slug: String) -> Result<String> {
    if MediaDao::find_by_ref(pool, &slug).await?.is_none() {
        return Ok(slug);
    }
    for n in 2..1000 {
        let candidate = format!("{slug}-{n}");
        if MediaDao::find_by_ref(pool, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(anyhow!("could not find a unique media ref for {slug:?}"))
}
