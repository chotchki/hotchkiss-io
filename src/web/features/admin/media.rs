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
use http::{header, StatusCode};
use serde_json::json;
use sqlx::SqlitePool;

use crate::db::dao::crypto_key::CryptoKey;
use crate::db::dao::media::{MediaDao, MediaKind, MediaVariantDao};
use crate::db::dao::roles::Role;
use crate::media::poster::generate_poster;
use crate::media::probe::{probe, Probed};
use crate::media::resize::{responsive_avif_variants, ResizeResult};
use crate::media::{media_url_key, MediaStore};
use crate::web::authentication_state::AuthenticationState;
use crate::web::features::media::{build_manifest, render_embed_html};
use crate::web::features::top_bar::TopBar;
use crate::web::htmx_responses::htmx_refresh;
use crate::web::{app_error::AppError, app_state::AppState, html_template::HtmlTemplate, session::SessionData};

/// `201 Created` + `Location` + the item manifest — the response for the DQ
/// server-assigns-identity creates (`POST /media`, `POST …/variants`). The manifest
/// is built with `Role::Admin` (the mutation gate guarantees the caller is Admin).
fn created_manifest(media: &MediaDao, variants: &[MediaVariantDao]) -> Response {
    let location = format!("/media/{}", media.media_ref);
    (
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(build_manifest(media, variants, Role::Admin)),
    )
        .into_response()
}

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
    /// The gate badge label (DC.5), from the fail-closed decode — `None` = public.
    pub visibility: Option<&'static str>,
    /// The decoded rank, for the selector's selected-option logic (0..=3).
    pub visibility_rank: u8,
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
        let visibility = m.visibility_label();
        let visibility_rank = m.min_role_rank();
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
            visibility,
            visibility_rank,
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
        top_bar: TopBar::create(&state.pool, "admin", session_data.auth_state.role()).await?,
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

/// The item kind when an upload GROUPS several files: a MODEL (STL/3MF) or VIDEO
/// beats an IMAGE, which beats a generic FILE. So a render image grouped with a
/// model/video is treated as its THUMBNAIL/poster, not the item's type — and the
/// kind is order-INDEPENDENT (it used to be just the first file's, so an
/// image-first group silently lost its viewer).
fn dominant_kind(kinds: &[MediaKind]) -> MediaKind {
    if kinds.contains(&MediaKind::Stl) {
        MediaKind::Stl
    } else if kinds.contains(&MediaKind::Video) {
        MediaKind::Video
    } else if kinds.contains(&MediaKind::Audio) {
        // An audiobook grouped with its cover image is an AUDIO item; the
        // image is its artwork/thumbnail (same rule as model/video vs image).
        MediaKind::Audio
    } else if kinds.contains(&MediaKind::Image) {
        MediaKind::Image
    } else {
        MediaKind::File
    }
}

/// A file part streamed to the content store + ffprobe'd at ingest.
struct IngestedFile {
    sha: String,
    probed: Probed,
    len: i64,
    root: String,
}

/// The non-file multipart fields a media upload may carry.
#[derive(Default)]
struct MediaTextFields {
    media_ref: Option<String>,
    title: Option<String>,
    min_role: Option<String>,
    first_filename: Option<String>,
}

/// Stream every file part straight to the content store (O(chunk) memory —
/// hashed + written as it arrives, NEVER buffered whole, so a multi-GB upload
/// works without OOMing) and ffprobe each stored file; collect the parsed text
/// fields alongside. Shared by `upload_media` (mint a new item) and
/// `patch_media_by_ref` (complete-replace an existing item's variants), so the
/// one streaming path can't drift between the two.
async fn ingest_multipart(
    store: &MediaStore,
    mut multipart: Multipart,
) -> Result<(Vec<IngestedFile>, MediaTextFields)> {
    let mut files = Vec::new();
    let mut fields = MediaTextFields::default();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow!("reading multipart: {e}"))?
    {
        if let Some(fname) = field.file_name().map(|s| s.to_string()) {
            let mut staged = store.stage().await?;
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
            let (sha, len, root) = staged.commit(store).await?;
            let root = root.to_string_lossy().into_owned();
            let probed =
                probe_stored(store.clone(), sha.clone(), fname.clone(), Some(root.clone())).await?;
            if fields.first_filename.is_none() {
                fields.first_filename = Some(fname);
            }
            files.push(IngestedFile { sha, probed, len: len as i64, root });
        } else {
            let name = field.name().unwrap_or("").to_string();
            let value = field.text().await.unwrap_or_default();
            match name.as_str() {
                "media_ref" if !value.trim().is_empty() => fields.media_ref = Some(value),
                "title" if !value.trim().is_empty() => fields.title = Some(value),
                // Visibility default (DC.5): a file dropped on a GATED page's
                // editor must inherit that page's gate, not mint public media —
                // editor-support.js sends the page's current visibility here.
                // Only the known gate roles are accepted; absent / "Public" /
                // anything else → public (which is what `fab publish` sends:
                // nothing). PATCH ignores this — it preserves the item's gate.
                "min_role" => fields.min_role = parse_media_visibility(&value),
                _ => {}
            }
        }
    }
    Ok((files, fields))
}

/// After the variant rows exist, generate the derived variants that depend on the
/// item's kind — width-stepped AVIFs for an image (srcset), a frame-grab poster
/// for video/audio (thumbnail + lock-screen artwork). Best-effort (each logs on
/// failure; the primary still serves). Shared by upload + PATCH so the two can't
/// drift; the derived variants carry no `min_role` of their own → they inherit
/// the item's gate.
async fn add_derived_variants(
    state: &AppState,
    hmac_key: &[u8],
    media_id: i64,
    kind: MediaKind,
    primary_sha: String,
) {
    if kind == MediaKind::Image {
        maybe_add_responsive_variants(state, hmac_key, media_id, primary_sha).await;
    } else if matches!(kind, MediaKind::Video | MediaKind::Audio) {
        maybe_add_poster(state, hmac_key, media_id, primary_sha).await;
    }
}

/// Ingest a multipart into a NEW item + its variants — the shared core of the
/// admin `upload_media` and DQ's `create_media` (`POST /media`). Streams + probes,
/// mints the opaque UUIDv7 ref, derives kind/dims from the dominant file, inserts
/// the variants, runs the best-effort derived variants (image srcset / A-V poster),
/// and returns the item + its FINAL variant set. `None` = no files in the upload
/// (the caller returns a `400`).
async fn ingest_new_item(
    state: &AppState,
    multipart: Multipart,
) -> Result<Option<(MediaDao, Vec<MediaVariantDao>)>> {
    let (ingested, fields) = ingest_multipart(&state.media_store, multipart).await?;
    if ingested.is_empty() {
        return Ok(None);
    }
    // The URL ref is an OPAQUE, unguessable token (NOT a slug) — the byte route is
    // already HMAC'd; this closes the slug-guess gap. The human name lives in
    // `title` (library display / search), derived from the filename when not given.
    let media_ref = uuid::Uuid::now_v7().simple().to_string();
    let title = fields
        .title
        .or(fields.media_ref)
        .or_else(|| fields.first_filename.as_deref().map(strip_media_suffixes))
        .filter(|s| !s.trim().is_empty());

    let hmac_key = CryptoKey::get_or_create(&state.pool, MEDIA_HMAC_KEY_ID)
        .await?
        .key_value;

    // The item's kind is the DOMINANT kind across the grouped files (a model/video
    // beats an image), NOT just the first file's. Dims/duration come from the first
    // variant OF that kind (an image grouped into a model must not set model dims).
    let kinds: Vec<MediaKind> = ingested.iter().map(|f| f.probed.kind).collect();
    let kind = dominant_kind(&kinds);
    let primary = ingested.iter().find(|f| f.probed.kind == kind).unwrap_or(&ingested[0]);
    let (primary_sha, primary_probed) = (primary.sha.clone(), &primary.probed);
    let media = MediaDao::create(
        &state.pool,
        media_ref,
        kind,
        title,
        primary_probed.width,
        primary_probed.height,
        primary_probed.duration_ms,
        fields.min_role,
        primary_probed.chapters.clone(),
    )
    .await?;

    for f in &ingested {
        create_variant(
            &state.pool,
            &hmac_key,
            media.media_id,
            f.sha.clone(),
            f.probed.mime.clone(),
            f.probed.codecs.clone(),
            f.len,
            Some(f.root.clone()),
            f.probed.width,
            f.probed.height,
        )
        .await?;
    }
    add_derived_variants(state, &hmac_key, media.media_id, kind, primary_sha).await;

    let variants = MediaVariantDao::find_by_media_id(&state.pool, media.media_id).await?;
    Ok(Some((media, variants)))
}

/// `POST /admin/media/upload` — the admin library's create (htmx). Returns the
/// `media_id` / `media_ref` / ready-to-paste markdown the editor JS inserts. The
/// canonical REST create is `create_media` (`POST /media`, DQ.2); both share
/// `ingest_new_item`, so they can't drift. (DR migrates the library onto that.)
pub async fn upload_media(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let Some((media, _variants)) = ingest_new_item(&state, multipart).await? else {
        return Ok((StatusCode::BAD_REQUEST, "No files in the upload").into_response());
    };
    Ok(Json(json!({
        "media_id": media.media_id,
        "media_ref": media.media_ref,
        "markdown": format!("![](/media/{})", media.media_ref),
    }))
    .into_response())
}

/// `POST /media` — the canonical REST create (DQ.2). Ingests a new item (server
/// mints the UUIDv7 ref) → `201 Created` + `Location: /media/<ref>` + the manifest.
/// Admin-gated FOR FREE by `require_admin_for_mutations` (a POST → the admin
/// fallback). Shares `ingest_new_item` with the admin `upload_media`.
pub async fn create_media(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    match ingest_new_item(&state, multipart).await? {
        Some((media, variants)) => Ok(created_manifest(&media, &variants)),
        None => Ok((StatusCode::BAD_REQUEST, "No files in the upload").into_response()),
    }
}

/// `PUT /media/<ref>/variants` — REPLACE the item's entire variant collection
/// (Phase DQ.1, re-verbed from DO's `PATCH /media/<ref>`; the fab-scad round-trip
/// SAVE). A logged-in Admin re-uploads the model's files; the variant set is
/// COMPLETELY REPLACED, keeping the item's identity (`media_ref` / `title` /
/// `min_role`) untouched BY CONSTRUCTION — it lives on the PARENT `/media/<ref>`,
/// not this collection. Multipart file parts typed by extension. Anything not
/// re-uploaded is dropped — the uploaded set is authoritative. Old blobs go cold
/// (content-addressed, no in-line sweep — same as delete). Returns the manifest.
///
/// Admin-gated FOR FREE by `require_admin_for_mutations` (a non-safe method → the
/// admin fallback) — NOT the `/admin` nest's `require_admin`.
pub async fn replace_media_variants(
    State(state): State<AppState>,
    Path(media_ref): Path<String>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let Some(item) = MediaDao::find_by_ref(&state.pool, &media_ref).await? else {
        return Ok((StatusCode::NOT_FOUND, "No such media item").into_response());
    };

    let (ingested, _fields) = ingest_multipart(&state.media_store, multipart).await?;
    // A complete replace needs at least one file — replacing to zero variants is a
    // DELETE, not a PATCH. Reject it so a fumbled upload can't blank the item.
    if ingested.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "No files in the upload").into_response());
    }

    let hmac_key = CryptoKey::get_or_create(&state.pool, MEDIA_HMAC_KEY_ID)
        .await?
        .key_value;

    // Re-derive the item's kind/dims from the NEW set exactly as upload does; the
    // identity (ref/title/min_role) is preserved by NOT re-writing those columns.
    let kinds: Vec<MediaKind> = ingested.iter().map(|f| f.probed.kind).collect();
    let kind = dominant_kind(&kinds);
    let primary = ingested.iter().find(|f| f.probed.kind == kind).unwrap_or(&ingested[0]);
    let primary_sha = primary.sha.clone();

    // Atomic swap: wipe the old variant set, insert the new one, re-derive facts —
    // so a mid-flight failure never leaves the item with a mix of old + new.
    let mut tx = state.pool.begin().await?;
    MediaVariantDao::delete_all_for_media(&mut *tx, item.media_id).await?;
    for f in &ingested {
        let url_key = media_url_key(&hmac_key, &f.sha)?;
        MediaVariantDao::create(
            &mut *tx,
            item.media_id,
            f.sha.clone(),
            url_key,
            f.probed.mime.clone(),
            f.probed.codecs.clone(),
            f.len,
            Some(f.root.clone()),
            f.probed.width,
            f.probed.height,
        )
        .await?;
    }
    MediaDao::update_facts(
        &mut *tx,
        item.media_id,
        kind,
        primary.probed.width,
        primary.probed.height,
        primary.probed.duration_ms,
        primary.probed.chapters.clone(),
    )
    .await?;
    tx.commit().await?;

    // Derived variants (image srcset / video-audio poster) — best-effort, and they
    // inherit the item's preserved gate (they carry no min_role of their own).
    add_derived_variants(&state, &hmac_key, item.media_id, kind, primary_sha).await;

    // Reflect the final variant set back (the manifest) so fab-gui can confirm the swap.
    let item = MediaDao::find_by_ref(&state.pool, &media_ref).await?.unwrap_or(item);
    let variants = MediaVariantDao::find_by_media_id(&state.pool, item.media_id).await?;
    Ok(Json(build_manifest(&item, &variants, Role::Admin)).into_response())
}

/// APPEND variants to an EXISTING item (Phase DQ.3 shared core) — stream + probe
/// each file part and insert it, DEDUP'ing bytes already on the item. **APPEND-ONLY**
/// (the DQ.3 decision): unlike upload/replace it does NOT run `add_derived_variants`
/// (no poster / no responsive srcset) and does NOT re-derive the item's kind/dims —
/// you're adding a SPECIFIC variant (another codec, a poster, a mesh LOD), not
/// re-ingesting the item (use `PUT …/variants` to replace + re-derive). Returns
/// whether any file part was seen, so the caller `400`s an empty body.
async fn append_variants(
    state: &AppState,
    media_id: i64,
    mut multipart: Multipart,
) -> Result<bool> {
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
            let probed =
                probe_stored(state.media_store.clone(), sha.clone(), fname, Some(root.clone())).await?;
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
    Ok(saw_file)
}

/// `POST /admin/media/{media_id}/encode` — append a variant to an item BY id (the
/// admin library, htmx). Shares `append_variants` with the canonical
/// `add_media_variant` (`POST /media/<ref>/variants`, DQ.3).
pub async fn add_encode(
    State(state): State<AppState>,
    Path(media_id): Path<i64>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    if !append_variants(&state, media_id, multipart).await? {
        return Ok((StatusCode::BAD_REQUEST, "No file in the upload").into_response());
    }
    // A file present but fully deduped is an idempotent no-op — still refresh.
    Ok(htmx_refresh())
}

/// `POST /media/<ref>/variants` — ADD one variant to an existing item (Phase DQ.3):
/// the `add_encode` semantics addressed BY ref. The server mints the content-addressed
/// `url_key`, so it's a POST → `201` + `Location` + the manifest. APPEND-ONLY (see
/// `append_variants`). Admin-gated by the mutation layer. `404` unknown ref, `400`
/// empty body.
pub async fn add_media_variant(
    State(state): State<AppState>,
    Path(media_ref): Path<String>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let Some(item) = MediaDao::find_by_ref(&state.pool, &media_ref).await? else {
        return Ok((StatusCode::NOT_FOUND, "No such media item").into_response());
    };
    if !append_variants(&state, item.media_id, multipart).await? {
        return Ok((StatusCode::BAD_REQUEST, "No file in the upload").into_response());
    }
    let variants = MediaVariantDao::find_by_media_id(&state.pool, item.media_id).await?;
    Ok(created_manifest(&item, &variants))
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

/// Accept ONLY the known gate roles as a media visibility value; everything
/// else — empty, "Public", garbage, absent — is public (`None`). Media has no
/// keep-the-existing-gate write path here: upload sets it at birth, and the
/// explicit per-item selector always submits its full choice.
fn parse_media_visibility(value: &str) -> Option<String> {
    match value.trim() {
        v @ ("Registered" | "Family" | "Admin") => Some(v.to_string()),
        _ => None,
    }
}

#[derive(serde::Deserialize)]
pub struct VisibilityForm {
    #[serde(default)]
    pub min_role: String,
}

/// `POST /admin/media/{media_id}/visibility` — the library's per-item gate
/// control (DC.5). "Public" (or anything unrecognized) clears; a known gate
/// role sets. Refreshes so the card's badge + selector re-render.
pub async fn set_media_visibility(
    State(state): State<AppState>,
    Path(media_id): Path<i64>,
    axum::Form(form): axum::Form<VisibilityForm>,
) -> Result<Response, AppError> {
    MediaDao::set_min_role(&state.pool, media_id, parse_media_visibility(&form.min_role)).await?;
    Ok(htmx_refresh())
}

/// The `PUT /media/<ref>` metadata body (DQ.4). An ABSENT field KEEPS the current
/// value — fail-safe: a partial write must never silently clear a title or,
/// security-critical, LOOSEN the gate (mirrors the DB.5 visibility rule).
#[derive(serde::Deserialize)]
pub struct MetadataBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub min_role: Option<String>,
}

/// `PUT /media/<ref>` — replace the item's writable metadata (DQ.4; the `rename` +
/// `visibility` merge). JSON `{title?, min_role?}`; an absent field keeps its value.
/// `min_role`: `"Public"` clears, a known role sets, absent/garbage keeps (never
/// silently loosens). Returns the manifest; `404` unknown ref.
pub async fn update_media_metadata(
    State(state): State<AppState>,
    Path(media_ref): Path<String>,
    Json(body): Json<MetadataBody>,
) -> Result<Response, AppError> {
    let Some(item) = MediaDao::find_by_ref(&state.pool, &media_ref).await? else {
        return Ok((StatusCode::NOT_FOUND, "No such media item").into_response());
    };
    if let Some(title) = &body.title {
        MediaDao::update_title(&state.pool, item.media_id, title).await?;
    }
    match body.min_role.as_deref().map(str::trim) {
        Some("Public") => MediaDao::set_min_role(&state.pool, item.media_id, None).await?,
        Some(v @ ("Registered" | "Family" | "Admin")) => {
            MediaDao::set_min_role(&state.pool, item.media_id, Some(v.to_string())).await?
        }
        _ => {} // absent / unrecognized → keep (never silently loosen)
    }
    let item = MediaDao::find_by_ref(&state.pool, &media_ref).await?.unwrap_or(item);
    let variants = MediaVariantDao::find_by_media_id(&state.pool, item.media_id).await?;
    Ok(Json(build_manifest(&item, &variants, Role::Admin)).into_response())
}

/// `DELETE /media/<ref>` — delete the item (CASCADE its variants; DQ.4). `204`, or
/// `404` for an unknown ref.
pub async fn delete_media_item(
    State(state): State<AppState>,
    Path(media_ref): Path<String>,
) -> Result<Response, AppError> {
    let Some(item) = MediaDao::find_by_ref(&state.pool, &media_ref).await? else {
        return Ok((StatusCode::NOT_FOUND, "No such media item").into_response());
    };
    MediaDao::delete_by_id(&state.pool, item.media_id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /media/<ref>/variants/<url_key>` — remove ONE variant (DQ.4). `204`, or
/// `404` if the ref OR the key (within that item) is unknown.
pub async fn delete_media_variant(
    State(state): State<AppState>,
    Path((media_ref, url_key)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let Some(item) = MediaDao::find_by_ref(&state.pool, &media_ref).await? else {
        return Ok((StatusCode::NOT_FOUND, "No such media item").into_response());
    };
    if MediaVariantDao::delete_by_url_key_in_item(&state.pool, item.media_id, &url_key).await? {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Ok((StatusCode::NOT_FOUND, "No such variant").into_response())
    }
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

/// Frame-grab a poster for a video and add it as an image variant. Audio items
/// reuse it: the same ffmpeg command extracts an attached_pic cover (Phase DD).
/// Best-effort: a failure is logged, not surfaced — the media plays without it.
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
) {
    let store = state.media_store.clone();
    let result: Result<()> = async {
        let path_store = store.clone();
        let resized = tokio::task::spawn_blocking(move || -> Result<ResizeResult> {
            let path = path_store
                .resolve_path(&original_sha, None)
                .ok_or_else(|| anyhow!("resize source {original_sha} not found in any media root"))?;
            responsive_avif_variants(&path)
        })
        .await
        .map_err(|e| anyhow!("resize task panicked: {e}"))??;

        for r in resized.variants {
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominant_kind_lets_a_model_or_video_beat_an_image() {
        use MediaKind::*;
        // A render image grouped with a model → the item is a viewable model, not
        // an image (order-independent).
        assert_eq!(dominant_kind(&[Image, Stl]), Stl);
        assert_eq!(dominant_kind(&[Stl, Image]), Stl);
        assert_eq!(dominant_kind(&[Image, Video]), Video);
        // An audiobook + its cover art → an Audio item, order-independent.
        assert_eq!(dominant_kind(&[Image, Audio]), Audio);
        assert_eq!(dominant_kind(&[Audio, Image]), Audio);
        // No model/video → image wins over a generic file; all-file → File.
        assert_eq!(dominant_kind(&[File, Image]), Image);
        assert_eq!(dominant_kind(&[File]), File);
        // Homogeneous groups keep their kind.
        assert_eq!(dominant_kind(&[Image, Image]), Image);
    }
}
