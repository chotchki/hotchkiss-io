//! Public media routes (Phase BZ): the byte serve route + the embed renderer.
//!
//! `/media/file/<url_key>` streams the stored bytes with HTTP range support
//! (`tower_http::ServeFile` → `206`/`Accept-Ranges`, needed for video seeking).
//! `url_key` is the HMAC token, NOT the content sha (so the route is not a
//! file-existence oracle), and the bytes are immutable (content-addressed) so we
//! cache them hard.
//!
//! `/media/embed/<ref>` is the HTMX swap target for `![](/media/<ref>)`: the
//! transformer emits a placeholder that GETs here on load; we look up the media
//! kind + variants and return the right element (same pattern as inline diagrams).

use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use http::{header, HeaderName, HeaderValue, StatusCode};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::db::dao::media::{MediaDao, MediaKind, MediaVariantDao, ModelFormat};
use crate::web::util::media_ref::UrlKey;
use crate::web::app_state::AppState;
use crate::web::session::SessionData;

/// In-flow image height cap (matches the markdown transformer's content images).
const MAX_IMAGE_HEIGHT_PX: u32 = 480;

pub fn media_router() -> Router<AppState> {
    Router::new()
        .route("/file/{url_key}", get(serve_media_file))
        .route("/embed/{media_ref}", get(render_media_embed))
        // Download by the author ref (one path segment — never collides with the
        // two-segment /file/ and /embed/ routes above). PATCH on the same identity
        // is the fab-scad round-trip SAVE (Phase DO): complete variant replace in
        // place, Admin-gated FOR FREE by `require_admin_for_mutations` (GET public,
        // PATCH → admin fallback). `DefaultBodyLimit::disable()` for the multi-GB
        // model set, same as the /admin/media/upload route.
        .route(
            "/{media_ref}",
            get(serve_media_by_ref)
                .patch(crate::web::features::admin::media::patch_media_by_ref)
                .layer(DefaultBodyLimit::disable()),
        )
}

/// Download route for the author ref: `GET /media/<media_ref>` → 302 to the item's
/// full-res bytes. The upload API hands `fab publish` a `media_ref` (not the byte
/// `url_key`), so its Downloads links use this form; resolve the ref → the LARGEST
/// variant (full-res) → the canonical `/media/file/<url_key>` byte route (which
/// owns range / caching / nosniff / content-disposition). A UUIDv7 ref is
/// unguessable, so this adds no enumeration oracle.
async fn serve_media_by_ref(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(media_ref): Path<String>,
) -> Response {
    let media = match MediaDao::find_by_ref(&state.pool, &media_ref).await {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "Not found").into_response(),
        Err(e) => {
            tracing::error!("media lookup by ref failed: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "media lookup failed").into_response();
        }
    };
    // Visibility gate (DC.3): denied ≡ the unknown-ref miss above — no oracle.
    if !media.is_visible_to(session_data.auth_state.role()) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let variants = MediaVariantDao::find_by_media_id(&state.pool, media.media_id)
        .await
        .unwrap_or_default();
    match variants.iter().max_by_key(|v| v.bytes) {
        Some(v) => Redirect::temporary(&format!("/media/file/{}", v.url_key)).into_response(),
        None => (StatusCode::NOT_FOUND, "no downloadable file for this media").into_response(),
    }
}

/// Stream the bytes for a variant, addressed by its public HMAC `url_key`. Range
/// requests are handled by `ServeFile` (206). Content is immutable → cache hard.
async fn serve_media_file(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(url_key): Path<String>,
    req: Request,
) -> Response {
    // The token is 64 lowercase hex (HMAC-SHA256) — the `UrlKey` newtype IS that
    // format gate (DJ.4), so a junk path can't reach the store and the type carries
    // the invariant to the DAO call below. A malformed key looks identical to a
    // non-existent file (no oracle).
    let Some(key) = UrlKey::parse(&url_key) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    // One query resolves the variant AND the strictest-wins gate rank across
    // every item sharing this url_key (DC.2 — see the DAO doc for why the
    // LIMIT-1 owner alone could leak on deduped bytes).
    let (variant, required_rank) =
        match MediaVariantDao::find_by_url_key_with_required_rank(&state.pool, key.as_str()).await {
            Ok(Some(v)) => v,
            Ok(None) => return (StatusCode::NOT_FOUND, "Not found").into_response(),
            Err(e) => {
                tracing::error!("media lookup by url_key failed: {e:?}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "media lookup failed").into_response();
            }
        };
    // Denied ≡ the unknown-key miss above — a leaked gated URL simply stops
    // working, with no existence oracle.
    if (session_data.auth_state.role().rank() as i64) < required_rank {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    // Resolve the on-disk path OFF the async runtime: a hint hit is one stat, a
    // NULL/stale hint scans every root, and a stat to an asleep/wedged external
    // drive can block for seconds — that must not pin a tokio worker (would
    // head-of-line-block unrelated requests). The admin upload path already
    // spawn_blocking's this; the serve hot path was the gap.
    //
    // BOUND it (CN.12): spawn_blocking keeps the stat off the worker, but the
    // REQUEST still waited — a wedged / TCC-blocked drive hung the serve 25s+ (the
    // v0.0.81 incident). A blocking stat can't be cancelled, so on timeout we
    // ABANDON the task (it leaks ONE blocking thread until the stat eventually
    // returns — logged loudly so the log viewer surfaces a wedged root) and fail
    // fast with 503. The file READ (ServeFile, below) only runs once this stat has
    // PASSED — i.e. the drive just answered — so the stat is the real chokepoint;
    // bounding it covers the wedged-drive case without timing out a live stream.
    const DISK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let store = state.media_store.clone();
    let sha = variant.sha256.clone();
    let hint = variant.storage_root.clone();
    let resolve = tokio::task::spawn_blocking(move || store.resolve_path(&sha, hint.as_deref()));
    let path = match tokio::time::timeout(DISK_TIMEOUT, resolve).await {
        Ok(Ok(Some(p))) => p,
        Ok(Ok(None)) => {
            tracing::warn!(
                "media variant {} resolves to no mounted root (drive offline?)",
                variant.variant_id
            );
            return (StatusCode::NOT_FOUND, "Not found").into_response();
        }
        Ok(Err(e)) => {
            tracing::error!("media resolve task panicked: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "media resolve failed").into_response();
        }
        Err(_elapsed) => {
            tracing::error!(
                "media resolve for variant {} timed out after {DISK_TIMEOUT:?} — wedged media root? \
                 (the abandoned blocking stat leaks a thread until it returns); serving 503",
                variant.variant_id
            );
            return (StatusCode::SERVICE_UNAVAILABLE, "media temporarily unavailable")
                .into_response();
        }
    };
    let mime: mime_guess::mime::Mime = variant
        .mime
        .parse()
        .unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM);

    match ServeFile::new_with_mime(&path, &mime).oneshot(req).await {
        Ok(r) => {
            let mut resp = r.into_response();
            let headers = resp.headers_mut();
            // Gated bytes cache PRIVATE (DC.4): content-addressing keeps the
            // browser cache safe + seek-friendly, but a shared cache must never
            // hold role-gated bytes. Public media keeps the shared-cacheable
            // policy unchanged.
            headers.insert(
                header::CACHE_CONTROL,
                if required_rank > 0 {
                    HeaderValue::from_static("private, max-age=31536000, immutable")
                } else {
                    HeaderValue::from_static("public, max-age=31536000, immutable")
                },
            );
            // Never let an uploaded file run as active content on our canonical
            // origin: the byte route is public + same-origin, and a generic
            // `MediaKind::File` carries an uploader-influenced mime (guessed from
            // the filename). `nosniff` always; force-download the executable types
            // (an admin-sourced .svg/.html would otherwise be stored-XSS). The
            // probe-verified image/video/stl kinds never hit `is_active_content_mime`
            // so they still render inline via the embed's own elements.
            headers.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            // CORP so the COOP/COEP cross-origin-isolated editor (`/3d/editor`,
            // require-corp) can fetch a model's SCAD/mesh bytes (Phase DN;
            // resurrects the deferred CW.4). `cross-origin` (NOT `same-origin`)
            // also keeps public media hotlinkable/embeddable elsewhere; it does
            // NOT bypass the `min_role` gate — that's enforced above via
            // `required_rank`, so a denied request never reaches here.
            headers.insert(
                HeaderName::from_static("cross-origin-resource-policy"),
                HeaderValue::from_static("cross-origin"),
            );
            if is_active_content_mime(&variant.mime) {
                headers.insert(
                    header::CONTENT_DISPOSITION,
                    HeaderValue::from_static("attachment"),
                );
            }
            resp
        }
        Err(e) => {
            tracing::error!("serving media file {path:?} failed: {e:?}");
            (StatusCode::INTERNAL_SERVER_ERROR, "serve failed").into_response()
        }
    }
}

/// Mimes a browser would EXECUTE in our origin if served inline — force these to
/// download so an uploaded file can't run as same-origin script. Only a generic
/// `MediaKind::File` (mime guessed from the upload filename) ever carries one; the
/// probe-verified image/video/stl kinds don't.
fn is_active_content_mime(mime: &str) -> bool {
    let m = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        m.as_str(),
        "text/html"
            | "application/xhtml+xml"
            | "image/svg+xml"
            | "application/xml"
            | "text/xml"
            | "application/javascript"
            | "text/javascript"
            | "application/ecmascript"
    ) || m.ends_with("+xml")
}

/// HTMX swap target: resolve a media ref to its rendered element.
async fn render_media_embed(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(media_ref): Path<String>,
) -> Response {
    let media = match MediaDao::find_by_ref(&state.pool, &media_ref).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            // Same no-store as the denial below — a header DIFFERENCE between
            // miss and denial would itself be the oracle.
            return embed_response(error_span("media not found — the page may need a reload"));
        }
        Err(e) => {
            tracing::error!("media embed lookup failed: {e:?}");
            return embed_response(error_span("media lookup failed"));
        }
    };
    // Visibility gate (DC.3): denied ≡ the bad-ref miss above, byte for byte —
    // still HTTP 200 so the HTMX swap lands, still no oracle.
    if !media.is_visible_to(session_data.auth_state.role()) {
        return embed_response(error_span("media not found — the page may need a reload"));
    }
    let variants = MediaVariantDao::find_by_media_id(&state.pool, media.media_id)
        .await
        .unwrap_or_default();
    embed_response(render_embed_html(&media, &variants))
}

/// Embed responses are ROLE-DEPENDENT HTML (element vs miss-span) — `no-store`
/// so no cache, shared or local, can ever hand one viewer's embed to another
/// (a cached Family embed would leak a gated url_key to anon: an existence
/// oracle; a cached anon miss-span would blank a Family view). HTMX refetches
/// per page load anyway, so nothing of value is lost.
fn embed_response(html: String) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Html(html)).into_response()
}

/// Build the element for a media item — the polymorphic dispatch on `kind`.
/// `pub(crate)` so the admin library can reuse the playable `<video>`.
///
/// GATE INVARIANT (DC): this fn must only ever reference bytes via
/// `/media/file/<url_key>` URLs — the byte route re-gates STRICTEST-WINS
/// across items sharing a url_key. The embed/302 handlers gate on the item's
/// OWN min_role (looser on deduped bytes), which is safe precisely because no
/// bytes are inlined here. Never emit a `data:` URI or server-read content
/// from this fn, or deduped gated bytes could leak through a public item.
pub(crate) fn render_embed_html(media: &MediaDao, variants: &[MediaVariantDao]) -> String {
    let alt = attr_escape(media.title.as_deref().unwrap_or(&media.media_ref));
    let kind = media.kind().unwrap_or(MediaKind::File);
    // The OpenSCAD source variant, if any (Phase DN) — the input fab-gui SLICES.
    // Surfaced as an "Open in the slicer" button in the Stl / File arms below.
    let scad = variants
        .iter()
        .find(|v| ModelFormat::from_mime(&v.mime) == Some(ModelFormat::Scad));
    match kind {
        MediaKind::Image => {
            let Some(fallback) = variants.first() else {
                return error_span("image has no stored file");
            };
            // Image variants carrying a width → a srcset (Phase CN): a new upload
            // records the original's width plus its 480/960 AVIF downscales, so the
            // browser pulls an appropriately-sized file instead of the full-res
            // original. A legacy image with no widths falls through to a single src.
            let mut sized: Vec<(&MediaVariantDao, i64)> = variants
                .iter()
                .filter(|v| v.mime.starts_with("image/"))
                .filter_map(|v| v.width.map(|w| (v, w)))
                .collect();
            sized.sort_by_key(|(_, w)| *w);
            // src = the largest (best for a no-srcset client + the zoom view);
            // falls back to the first variant when nothing has a width.
            let src_key = sized
                .last()
                .map(|(v, _)| v.url_key.as_str())
                .unwrap_or(fallback.url_key.as_str());
            // Only worth a srcset with ≥2 distinct sizes.
            let srcset_attr = if sized.len() >= 2 {
                let entries: Vec<String> = sized
                    .iter()
                    .map(|(v, w)| format!("/media/file/{} {w}w", v.url_key))
                    .collect();
                format!(
                    " srcset=\"{}\" sizes=\"(max-width: 768px) 100vw, 768px\"",
                    entries.join(", ")
                )
            } else {
                String::new()
            };
            format!(
                "<img class=\"content-image mx-auto my-4 block cursor-zoom-in\" \
style=\"max-width:100%;max-height:{MAX_IMAGE_HEIGHT_PX}px\" data-zoomable=\"true\" tabindex=\"0\" \
role=\"button\" aria-label=\"Zoom image\" src=\"/media/file/{src_key}\"{srcset_attr} alt=\"{alt}\" />"
            )
        }
        MediaKind::Video => {
            // Video/* variants are playback sources; an image/* variant is the
            // poster (below). Order sources by HARDWARE-decode likelihood: HEVC
            // first (Apple HW-decodes it; most Macs/iPhones lack AV1 HW and
            // software-decode AV1 → dropped frames), then AV1, then the rest. The
            // browser picks the FIRST source it can play, so the order decides
            // which decoder it uses — getting this wrong played back jerky.
            let mut vids: Vec<&MediaVariantDao> = variants
                .iter()
                .filter(|v| v.mime.starts_with("video/"))
                .collect();
            vids.sort_by_key(|v| codec_rank(v.codecs.as_deref()));
            let mut sources = String::new();
            for v in vids {
                // `type` is single-quoted and the mime/codecs are server-derived
                // (ffprobe-mapped, safe charset), so no escaping — keeps the inner
                // `codecs="…"` double quotes intact.
                let type_attr = match &v.codecs {
                    Some(c) => format!("{}; codecs=\"{}\"", v.mime, c),
                    None => v.mime.clone(),
                };
                sources.push_str(&format!(
                    "<source src=\"/media/file/{}\" type='{type_attr}'>",
                    v.url_key
                ));
            }
            let poster = variants
                .iter()
                .rev()
                .find(|v| v.mime.starts_with("image/"))
                .map(|v| format!(" poster=\"/media/file/{}\"", v.url_key))
                .unwrap_or_default();
            let dims = match (media.width, media.height) {
                (Some(w), Some(h)) => format!(" width=\"{w}\" height=\"{h}\""),
                _ => String::new(),
            };
            format!(
                // Cap the inline player at a comfortable width (centered, aspect
                // preserved via the width/height attrs) so a big source doesn't
                // dominate the page; `controls` gives a native fullscreen button
                // for anyone who wants it larger. Mirrors the 480px image cap.
                "<video class=\"media-video mx-auto my-4 block w-full max-w-2xl h-auto rounded-md border-4 border-navy\" \
controls preload=\"metadata\" playsinline{poster}{dims}>{sources}\
Your browser can't play this video.</video>"
            )
        }
        MediaKind::Stl => {
            // Select ONLY over MESH variants (STL/3MF via `ModelFormat::is_mesh`) —
            // an image variant is a THUMBNAIL/poster and the SCAD source is the
            // SLICER input, NEVER the mesh, so neither may be mis-picked as the
            // viewer or download (Phase DN; scad is `application/…`, so the old
            // `model/*` glob already excluded it — this makes it explicit + typed).
            let models: Vec<&MediaVariantDao> = variants
                .iter()
                .filter(|v| ModelFormat::from_mime(&v.mime).is_some_and(ModelFormat::is_mesh))
                .collect();
            if models.is_empty() {
                return error_span("stl has no stored mesh");
            }
            // Multiple model variants = LEVELS OF DETAIL (decimated + full-res) and/or
            // formats (image + low-res 3MF + high-res 3MF is the common set). Pick by
            // TYPE then SIZE:
            //   VIEWER   = the SMALLEST 3MF (color + fast to fetch/render); if there's
            //              no 3MF, the smallest mesh (the STL LOD preview).
            //   DOWNLOAD = the LARGEST mesh (full-res, printable).
            // `bytes` is the fidelity key — decimation is monotonic (low-res < full),
            // and `variant_id` fetch order is insertion order (NOT fidelity), so the
            // min/max-by-bytes is load-bearing. A single-variant model views +
            // downloads the same file.
            let smallest_3mf = models
                .iter()
                .copied()
                .filter(|v| v.mime == "model/3mf")
                .min_by_key(|v| v.bytes);
            let viewer = smallest_3mf
                .unwrap_or_else(|| models.iter().copied().min_by_key(|v| v.bytes).unwrap());
            let fmt = if viewer.mime == "model/3mf" { "3mf" } else { "stl" };
            let full = models.iter().copied().max_by_key(|v| v.bytes).unwrap();
            // If the model carries its OpenSCAD source, offer to open it in the
            // slicer (Phase DN) — under the viewer + full-res download.
            let slicer = scad
                .map(|v| open_in_slicer_button(&v.url_key))
                .unwrap_or_default();
            format!(
                "<span class=\"flex flex-col items-center gap-2 my-4\">{}{}{}</span>",
                stl_viewer_block(&format!("/media/file/{}", viewer.url_key), fmt),
                download_button(&full.url_key, &alt, full.bytes),
                slicer,
            )
        }
        MediaKind::Audio => {
            // Playback sources = the audio/* variants (universal codecs by
            // construction — the probe bails to MediaKind::File otherwise).
            // An image variant is the ARTWORK (MediaSession / cards), never a
            // source. Degrades to the bare native <audio controls> without JS;
            // audio-player.js enhances via the data-* attributes (chapters,
            // skips, rate, MediaSession, resume — Phase DD).
            let audios: Vec<&MediaVariantDao> = variants
                .iter()
                .filter(|v| v.mime.starts_with("audio/"))
                .collect();
            if audios.is_empty() {
                return error_span("audio has no stored stream");
            }
            let mut sources = String::new();
            for v in &audios {
                sources.push_str(&format!(
                    "<source src=\"/media/file/{}\" type=\"{}\">",
                    v.url_key, v.mime
                ));
            }
            let artwork_url = variants
                .iter()
                .rev()
                .find(|v| v.mime.starts_with("image/"))
                .map(|v| format!("/media/file/{}", v.url_key));
            let artwork_attr = artwork_url
                .as_ref()
                .map(|u| format!(" data-artwork=\"{u}\""))
                .unwrap_or_default();
            let chapters_attr = media
                .chapters
                .as_deref()
                .map(|c| format!(" data-chapters=\"{}\"", attr_escape(c)))
                .unwrap_or_default();
            // Header = cover art + title, NOT a download button (a series page
            // of N volumes was N navy download slabs — the wrong emphasis for
            // listeners; the bytes stay reachable via the library's Copy link).
            // The <img> is decorative (alt="") — the adjacent title names it.
            let cover_img = artwork_url
                .as_ref()
                .map(|u| {
                    format!(
                        "<img class=\"size-24 rounded-md border-2 border-navy object-cover shrink-0\" \
src=\"{u}\" alt=\"\" loading=\"lazy\" />"
                    )
                })
                .unwrap_or_default();
            format!(
                "<span class=\"audio-embed flex flex-col items-center gap-2 my-4 w-full max-w-2xl mx-auto\">\
<span class=\"flex flex-row items-center gap-3 w-full\">{cover_img}\
<span class=\"font-display text-navy text-lg\">{alt}</span></span>\
<audio class=\"w-full\" controls preload=\"metadata\" data-ref=\"{media_ref}\" data-title=\"{alt}\"{chapters_attr}{artwork_attr}>{sources}\
Your browser can't play this audio.</audio></span>",
                media_ref = attr_escape(&media.media_ref),
            )
        }
        MediaKind::File => {
            let Some(v) = variants.first() else {
                return error_span("file has no content");
            };
            // A standalone OpenSCAD source (no grouped mesh): lead with the slicer
            // button — the point — and keep a download for the raw source (Phase DN).
            // Any other file is just a download.
            if let Some(s) = scad {
                format!(
                    "<span class=\"flex flex-col items-center gap-2 my-4\">{}{}</span>",
                    open_in_slicer_button(&s.url_key),
                    download_button(&v.url_key, &alt, v.bytes),
                )
            } else {
                download_button(&v.url_key, &alt, v.bytes)
            }
        }
    }
}

/// Inline fullscreen (expand-to-corners) glyph for the STL viewer's zoom button.
const FULLSCREEN_ICON_SVG: &str = "<svg viewBox=\"0 0 16 16\" width=\"1em\" height=\"1em\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M6 2H2v4\"/><path d=\"M10 2h4v4\"/><path d=\"M6 14H2v-4\"/><path d=\"M10 14h4v-4\"/></svg>";

/// The interactive STL viewer block: the three.js `<object>` (sized like a content
/// image/diagram — full width to a cap, `h-96`, centered by the caller) plus a
/// fullscreen toggle overlaid top-right. `htmx-stl-view.js` renders into the
/// `.stl-view` object and binds the `.stl-fullscreen` button (fullscreens the
/// `.stl-embed` wrapper so the button stays reachable; three.js re-sizes on
/// `fullscreenchange`). Shared by the `/media` embed and the transformer's `.stl`
/// rewrite so both look identical. `data_filename` is attr-escaped (the transformer
/// path carries an author-supplied URL).
pub(crate) fn stl_viewer_block(data_filename: &str, format: &str) -> String {
    // `<span>` (not `<div>`) with a `block` display: a standalone `![](x.stl)` is
    // parsed inside a `<p>`, and a block `<div>` there is invalid (auto-closes the
    // p). A span is phrasing content — valid in a paragraph — and `<object>` +
    // `<button>` are too. `data-format` (stl|3mf) tells the viewer which loader to
    // use, since the `/media/file/<url_key>` URL carries no extension.
    format!(
        "<span class=\"stl-embed relative block w-full max-w-2xl\">\
<object class=\"stl-view block w-full h-96 rounded-md border-4 border-navy\" data-filename=\"{url}\" data-format=\"{fmt}\"></object>\
<button type=\"button\" class=\"stl-fullscreen absolute top-2 right-2 bg-navy/80 text-div-grey rounded p-2 leading-none hover:bg-navy\" title=\"View fullscreen\" aria-label=\"View fullscreen\">{FULLSCREEN_ICON_SVG}</button>\
</span>",
        url = attr_escape(data_filename),
        fmt = attr_escape(format),
    )
}

/// A styled download BUTTON (glyph + name + human size), not a bare link — the
/// affordance for a 3MF / OpenSCAD source / zip, and the full-res STL under a
/// viewer. `download` forces save-to-disk intent (the byte route only
/// force-attaches executable mimes; a 3MF would otherwise open inline). `label`
/// MUST already be attr-escaped by the caller.
fn download_button(url_key: &str, label: &str, bytes: i64) -> String {
    format!(
        "<a class=\"media-download inline-flex items-center gap-2 my-2 px-4 py-2 bg-navy text-div-grey rounded no-underline hover:bg-navy/90\" \
href=\"/media/file/{url_key}\" download=\"{label}\">{DOWNLOAD_ICON_SVG}<span>Download {label} <span class=\"opacity-70\">({size})</span></span></a>",
        size = human_bytes(bytes),
    )
}

/// Inline download glyph for the file button (arrow into a tray). The embed HTML is
/// built as a Rust string, not askama, so the `icons::*` macros aren't reachable
/// here — this is the equivalent hand-inlined SVG, sized to the text (`1em`).
const DOWNLOAD_ICON_SVG: &str = "<svg viewBox=\"0 0 16 16\" width=\"1em\" height=\"1em\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M8 2v8\"/><path d=\"M4.5 7 8 10.5 11.5 7\"/><path d=\"M2.5 13.5h11\"/></svg>";

/// Inline "stacked layers" glyph for the Open-in-the-slicer button (a sliced model).
const SLICER_ICON_SVG: &str = "<svg viewBox=\"0 0 16 16\" width=\"1em\" height=\"1em\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M8 1.5 14.5 5 8 8.5 1.5 5 8 1.5Z\"/><path d=\"M1.5 8 8 11.5 14.5 8\"/><path d=\"M1.5 11 8 14.5 14.5 11\"/></svg>";

/// "Open in the slicer" button (Phase DN): hands fab-gui the SCAD source URL via
/// its `?model=` deep-link. Root-relative so fab-gui's `fetch_text` resolves it
/// same-origin (the editor is COEP-isolated; the byte route carries CORP). Yellow
/// (not the navy download) so the "open the tool" action reads distinct from
/// "download the file". The url_key is 64-hex — no query metacharacters to escape.
fn open_in_slicer_button(scad_url_key: &str) -> String {
    format!(
        "<a class=\"media-slice inline-flex items-center gap-2 my-2 px-4 py-2 bg-yellow text-navy rounded no-underline hover:bg-yellow/90\" \
href=\"/3d/editor?model=/media/file/{scad_url_key}\">{SLICER_ICON_SVG}<span>Open in the slicer</span></a>"
    )
}

/// Human-readable byte size for the download button (1024-based). Bytes below 1 KiB
/// stay exact; KB whole, MB/GB one decimal.
fn human_bytes(bytes: i64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < K * K {
        format!("{:.0} KB", b / K)
    } else if b < K * K * K {
        format!("{:.1} MB", b / (K * K))
    } else {
        format!("{:.1} GB", b / (K * K * K))
    }
}

fn error_span(msg: &str) -> String {
    format!(
        "<span class=\"media-error text-red-700 italic\">{}</span>",
        attr_escape(msg)
    )
}

/// The cover image URL for a page (Phase BZ.8): its `page_cover_media_id`'s first
/// image variant — the image itself for an image cover, the poster for a video.
/// `None` when no cover is set or it has no image variant. Used by the blog +
/// project card indexes, replacing the old `/attachments/id/<n>` cover render.
pub(crate) async fn cover_url_for(pool: &sqlx::SqlitePool, page_id: i64) -> Option<String> {
    // Prefer the SMALLEST width-stepped variant (Phase CN): a card thumbnail is
    // ~300px, so serving the 480px AVIF beats the full-res original. NULL-width
    // variants (legacy / the original) sort last → still picked when no resize
    // exists yet.
    sqlx::query_scalar!(
        r#"SELECT v.url_key FROM content_pages c
           JOIN media_variant v ON v.media_id = c.page_cover_media_id
           WHERE c.page_id = ?1 AND v.mime LIKE 'image/%'
           ORDER BY (v.width IS NULL), v.width ASC, v.variant_id LIMIT 1"#,
        page_id
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|k| format!("/media/file/{k}"))
}

/// A page's cover rendered as a hero (Phase CV): the LARGEST width-stepped image
/// variant — contrast `cover_url_for`, which serves the SMALLEST for a ~300px card
/// thumbnail — plus a `srcset` so a phone still pulls a smaller step. Rendered as a
/// stacked banner at the top of the detail view.
pub(crate) struct CoverHero {
    /// `/media/file/<url_key>` of the largest image variant (the no-srcset src).
    pub src: String,
    /// `"…480w, …960w"` when the cover has ≥2 sized variants; `None` for a legacy
    /// (unresized) cover, which then renders a single `src`.
    pub srcset: Option<String>,
}

/// `None` when the page has no cover or the cover has no image variant.
pub(crate) async fn cover_hero_for(pool: &sqlx::SqlitePool, page_id: i64) -> Option<CoverHero> {
    // Same cover join as `cover_url_for`, but LARGEST-first: a hero is a full-width
    // banner, so the biggest sized AVIF is the src (a NULL-width original sorts
    // last, so a real resize wins), with the sized variants as the srcset.
    let rows = sqlx::query!(
        r#"SELECT v.url_key AS "url_key!", v.width AS "width?: i64"
           FROM content_pages c
           JOIN media_variant v ON v.media_id = c.page_cover_media_id
           WHERE c.page_id = ?1 AND v.mime LIKE 'image/%'
           ORDER BY (v.width IS NULL), v.width DESC, v.variant_id"#,
        page_id
    )
    .fetch_all(pool)
    .await
    .ok()?;
    let largest = rows.first()?;
    let src = format!("/media/file/{}", largest.url_key);
    let sized: Vec<String> = rows
        .iter()
        .filter_map(|r| r.width.map(|w| format!("/media/file/{} {w}w", r.url_key)))
        .collect();
    let srcset = (sized.len() >= 2).then(|| sized.join(", "));
    Some(CoverHero { src, srcset })
}

/// The current cover's media REF (token) for a page, for the editor's cover field
/// pre-fill. `None` when no cover is set.
pub(crate) async fn cover_ref_for(pool: &sqlx::SqlitePool, page_id: i64) -> Option<String> {
    sqlx::query_scalar!(
        r#"SELECT m.media_ref FROM content_pages c
           JOIN media m ON m.media_id = c.page_cover_media_id
           WHERE c.page_id = ?1"#,
        page_id
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Resolve a pasted "Cover (media ref)" value → a `media_id`, tolerating every
/// shape the media library actually hands you: `![](/media/<ref>)` ("Copy ![]()"),
/// `/media/file/<url_key>` ("Copy link"), a bare `/media/<ref>`, or a bare ref.
/// The editor field demands a "media ref" but offers no bare-ref copy button, so
/// without this the natural copy-paste silently fails to set the cover.
///
/// `Ok(None)` means "couldn't resolve" — an empty field OR a non-empty typo. The
/// caller distinguishes them: an empty field clears the cover; an unresolvable
/// non-empty value is left alone so a typo can't wipe an existing cover.
pub(crate) async fn resolve_cover_media_id(
    pool: &sqlx::SqlitePool,
    raw: &str,
) -> Option<i64> {
    use crate::web::util::media_ref::{parse_cover_reference, MediaReference};
    match parse_cover_reference(raw)? {
        MediaReference::Ref(r) => MediaDao::find_by_ref(pool, r.as_str())
            .await
            .ok()
            .flatten()
            .map(|m| m.media_id),
        MediaReference::UrlKey(k) => MediaVariantDao::find_by_url_key(pool, k.as_str())
            .await
            .ok()
            .flatten()
            .map(|v| v.media_id),
    }
}

/// `<source>` ordering preference by hardware-decode likelihood (lower first):
/// HEVC (Apple HW) → AV1 (royalty-free, but software-decoded on most devices) →
/// H.264 → unknown. The browser plays the first source it can decode, so this
/// steers it toward a hardware path where one exists.
fn codec_rank(codecs: Option<&str>) -> u8 {
    match codecs {
        Some(c) if c.starts_with("hvc1") || c.starts_with("hev1") => 0,
        Some(c) if c.starts_with("av01") => 1,
        Some(c) if c.starts_with("avc1") => 2,
        _ => 3,
    }
}

/// Minimal HTML-attribute escaping for values we interpolate into tags.
fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(kind: &str) -> MediaDao {
        MediaDao {
            media_id: 1,
            media_ref: "intro".to_string(),
            kind: kind.to_string(),
            title: None,
            width: Some(1728),
            height: Some(1116),
            duration_ms: Some(44_908),
            created_at: "now".to_string(),
            min_role: None,
            chapters: None,
        }
    }
    fn variant(url_key: &str, mime: &str, codecs: Option<&str>) -> MediaVariantDao {
        MediaVariantDao {
            variant_id: 1,
            media_id: 1,
            sha256: "sha".to_string(),
            url_key: url_key.to_string(),
            mime: mime.to_string(),
            codecs: codecs.map(|c| c.to_string()),
            bytes: 100,
            storage_root: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn video_renders_multi_source_with_poster() {
        let variants = vec![
            variant("av1key", "video/mp4", Some("av01.0.12M.08")),
            variant("hevckey", "video/mp4", Some("hvc1")),
            variant("posterkey", "image/avif", None), // the auto-poster
        ];
        let html = render_embed_html(&media("video"), &variants);
        assert!(html.contains("<video"), "{html}");
        assert!(html.contains("poster=\"/media/file/posterkey\""), "{html}");
        assert!(
            html.contains("src=\"/media/file/av1key\" type='video/mp4; codecs=\"av01.0.12M.08\"'"),
            "{html}"
        );
        assert!(html.contains("src=\"/media/file/hevckey\" type='video/mp4; codecs=\"hvc1\"'"), "{html}");
        // the poster image is the <video poster>, NOT a playback <source>
        assert!(!html.contains("<source src=\"/media/file/posterkey\""), "{html}");
        // HEVC is ordered BEFORE AV1 (Apple hardware-decodes HEVC → smooth),
        // even though AV1 was the first variant.
        assert!(
            html.find("hevckey").unwrap() < html.find("av1key").unwrap(),
            "HEVC source must come first: {html}"
        );
    }

    #[test]
    fn image_renders_zoomable_img() {
        let html = render_embed_html(&media("image"), &[variant("imgkey", "image/avif", None)]);
        assert!(html.contains("<img"), "{html}");
        assert!(html.contains("src=\"/media/file/imgkey\""), "{html}");
        assert!(html.contains("data-zoomable"), "{html}");
    }

    #[test]
    fn image_with_width_variants_renders_srcset() {
        let mut orig = variant("korig", "image/png", None);
        orig.width = Some(3000);
        let mut v480 = variant("k480", "image/avif", None);
        v480.width = Some(480);
        let mut v960 = variant("k960", "image/avif", None);
        v960.width = Some(960);
        // Insert order shuffled — the render sorts by width.
        let html = render_embed_html(&media("image"), &[v960, orig, v480]);
        assert!(html.contains("srcset="), "{html}");
        assert!(html.contains("/media/file/k480 480w"), "{html}");
        assert!(html.contains("/media/file/k960 960w"), "{html}");
        assert!(html.contains("/media/file/korig 3000w"), "{html}");
        assert!(html.contains("sizes="), "{html}");
        // src = the largest, for a no-srcset client + the zoom view.
        assert!(html.contains("src=\"/media/file/korig\""), "{html}");
    }

    #[test]
    fn legacy_image_without_widths_has_no_srcset() {
        // A pre-CN image (no recorded widths) degrades to a single <img src>.
        let html = render_embed_html(&media("image"), &[variant("only", "image/png", None)]);
        assert!(html.contains("src=\"/media/file/only\""), "{html}");
        assert!(!html.contains("srcset="), "{html}");
    }

    #[test]
    fn stl_renders_object_viewer() {
        let html = render_embed_html(&media("stl"), &[variant("stlkey", "model/stl", None)]);
        assert!(html.contains("<object class=\"stl-view"), "{html}");
        assert!(html.contains("data-filename=\"/media/file/stlkey\""), "{html}");
        // A single-variant STL still offers a download of that same file.
        assert!(html.contains("href=\"/media/file/stlkey\""), "single STL is downloadable: {html}");
        // Fullscreen zoom affordance is present.
        assert!(html.contains("stl-fullscreen"), "has the fullscreen zoom button: {html}");
    }

    #[test]
    fn stl_with_lod_variants_previews_small_downloads_full() {
        // fab publish provides a decimated mesh + the full-res, as two variants of
        // one item. Insert order is full-then-decimated on purpose — the render must
        // pick by BYTES, not insertion order.
        let mut full = variant("fullkey", "model/stl", None);
        full.bytes = 5_000_000; // ~4.8 MB full-res
        let mut decimated = variant("previewkey", "model/stl", None);
        decimated.bytes = 50_000; // decimated
        let html = render_embed_html(&media("stl"), &[full, decimated]);
        assert!(
            html.contains("data-filename=\"/media/file/previewkey\""),
            "viewer loads the smallest (decimated) mesh: {html}"
        );
        assert!(
            html.contains("href=\"/media/file/fullkey\""),
            "download offers the largest (full-res): {html}"
        );
        assert!(html.contains("(4.8 MB)"), "download shows the full-res size: {html}");
        assert!(html.contains("data-format=\"stl\""), "an all-STL item views as stl: {html}");
    }

    #[test]
    fn model_item_that_is_only_a_3mf_renders_a_color_viewer() {
        // A model published as a single colored 3MF (bowtie's shape). The item's
        // kind is Stl (probe types .3mf as viewable), so it must render the VIEWER
        // in 3mf format — not a bare download.
        let html = render_embed_html(&media("stl"), &[variant("mfkey", "model/3mf", None)]);
        assert!(html.contains("<object class=\"stl-view"), "renders a viewer, not a download: {html}");
        assert!(html.contains("data-filename=\"/media/file/mfkey\""), "{html}");
        assert!(html.contains("data-format=\"3mf\""), "loads via 3MFLoader: {html}");
        assert!(html.contains("href=\"/media/file/mfkey\""), "the 3mf is also downloadable: {html}");
    }

    #[test]
    fn model_with_scad_variant_offers_open_in_slicer() {
        // A model carrying a mesh + its OpenSCAD source (Phase DN): the mesh still
        // drives the viewer/download, and the scad adds an "Open in the slicer"
        // button — the scad must NEVER be mis-picked as the mesh.
        let mut mesh = variant("meshkey", "model/3mf", None);
        mesh.bytes = 400_000;
        let scad = variant("scadkey", "application/x-openscad", None);
        let html = render_embed_html(&media("stl"), &[mesh, scad]);
        assert!(
            html.contains("data-filename=\"/media/file/meshkey\""),
            "viewer is the mesh: {html}"
        );
        assert!(
            !html.contains("data-filename=\"/media/file/scadkey\""),
            "scad is never loaded into the mesh viewer: {html}"
        );
        assert!(
            html.contains("href=\"/3d/editor?model=/media/file/scadkey\""),
            "Open-in-the-slicer targets the scad source: {html}"
        );
        assert!(html.contains("Open in the slicer"), "{html}");
    }

    #[test]
    fn standalone_scad_file_offers_slicer_and_download() {
        // A `.scad` uploaded alone probes as a File (application/x-openscad); its
        // embed leads with the slicer button and keeps a source download — not a
        // bare download (Phase DN).
        let html = render_embed_html(
            &media("file"),
            &[variant("scadkey", "application/x-openscad", None)],
        );
        assert!(
            html.contains("href=\"/3d/editor?model=/media/file/scadkey\""),
            "standalone scad opens in the slicer: {html}"
        );
        assert!(html.contains("Open in the slicer"), "{html}");
        assert!(
            html.contains("href=\"/media/file/scadkey\""),
            "raw source still downloadable: {html}"
        );
    }

    #[test]
    fn multicolor_model_views_low_3mf_downloads_high_3mf_with_slicer() {
        // A MULTICOLOR model can't use a low-res STL for the web viewer (STL carries
        // no color), so it ships SCAD + low-res 3MF + high-res 3MF. The viewer picks
        // the SMALLEST 3MF (color + fast), the download the LARGEST, the slicer the
        // source — all from one item.
        let mut low = variant("low3mf", "model/3mf", None);
        low.bytes = 120_000;
        let mut high = variant("high3mf", "model/3mf", None);
        high.bytes = 2_400_000;
        let scad = variant("scadkey", "application/x-openscad", None);
        // Shuffled insert order — selection is by type+size, never order.
        let html = render_embed_html(&media("stl"), &[high, scad, low]);
        assert!(
            html.contains("data-filename=\"/media/file/low3mf\""),
            "viewer = the low-res 3MF (color + fast): {html}"
        );
        assert!(html.contains("data-format=\"3mf\""), "viewer loads via 3MFLoader (color): {html}");
        assert!(
            html.contains("href=\"/media/file/high3mf\""),
            "download = the high-res 3MF: {html}"
        );
        assert!(
            html.contains("href=\"/3d/editor?model=/media/file/scadkey\""),
            "slicer button targets the source: {html}"
        );
    }

    #[test]
    fn model_with_image_and_two_3mfs_views_low_downloads_high() {
        // The real fab set: a render image + a low-res 3MF + a high-res 3MF. Viewer =
        // the SMALLEST 3MF (color + fast); download = the LARGEST mesh; image = neither.
        let mut img = variant("imgkey", "image/avif", None);
        img.bytes = 20_000;
        let mut low = variant("lowkey", "model/3mf", None);
        low.bytes = 120_000;
        let mut high = variant("highkey", "model/3mf", None);
        high.bytes = 2_400_000;
        // Insert order shuffled — selection is by type+size, not order.
        let html = render_embed_html(&media("stl"), &[high, img, low]);
        assert!(html.contains("data-filename=\"/media/file/lowkey\""), "viewer = low-res 3MF: {html}");
        assert!(html.contains("data-format=\"3mf\""), "{html}");
        assert!(html.contains("href=\"/media/file/highkey\""), "download = high-res 3MF: {html}");
        assert!(!html.contains("imgkey"), "image is the thumbnail, not viewer/download: {html}");
    }

    #[test]
    fn model_item_ignores_image_variant_for_viewer_and_download() {
        // A model item can carry a render IMAGE (its library/card thumbnail). The
        // viewer + download must select over MODEL variants only — never load the
        // image as the mesh.
        let mut render = variant("renderkey", "image/avif", None);
        render.bytes = 20_000; // smaller than the stl — must NOT be picked as viewer
        let mut stl = variant("meshkey", "model/stl", None);
        stl.bytes = 400_000;
        let html = render_embed_html(&media("stl"), &[render, stl]);
        assert!(html.contains("data-filename=\"/media/file/meshkey\""), "viewer is the mesh, not the image: {html}");
        assert!(html.contains("href=\"/media/file/meshkey\""), "download is the mesh: {html}");
        assert!(!html.contains("renderkey"), "the image variant is not the viewer or download: {html}");
    }

    #[test]
    fn stl_item_with_3mf_variant_views_in_color_downloads_full_stl() {
        // fab publishes a COLORED 3MF alongside the STL LOD, all one item. The viewer
        // prefers the 3MF (STL has no color); the download offers the full-res STL.
        let mut decimated = variant("deckey", "model/stl", None);
        decimated.bytes = 50_000;
        let mut full = variant("fullstlkey", "model/stl", None);
        full.bytes = 5_000_000;
        let mut colored = variant("colorkey", "model/3mf", None);
        colored.bytes = 800_000;
        let html = render_embed_html(&media("stl"), &[decimated, full, colored]);
        assert!(html.contains("data-filename=\"/media/file/colorkey\""), "viewer uses the 3MF: {html}");
        assert!(html.contains("data-format=\"3mf\""), "viewer format is 3mf: {html}");
        assert!(
            html.contains("href=\"/media/file/fullstlkey\""),
            "download offers the full-res STL: {html}"
        );
    }

    /// DD.2 + DG.5: the audio arm — native <audio> with universal sources, the
    /// data-* contract the player enhances, artwork excluded from sources, and
    /// a cover+title header instead of a download button (series-page feedback:
    /// N volumes were N download slabs).
    #[test]
    fn audio_renders_player_element_with_chapters_and_cover_header() {
        let mut m = media("audio");
        m.title = Some("Test Book".to_string());
        m.chapters = Some(r#"[{"start_ms":0,"title":"One"}]"#.to_string());
        let mut full = variant("fullaudio", "audio/mp4", None);
        full.bytes = 900_000;
        let mut low = variant("lowaudio", "audio/mp4", None);
        low.bytes = 100_000;
        let mut art = variant("artkey", "image/avif", None);
        art.bytes = 10_000;
        let html = render_embed_html(&m, &[low.clone(), full, art]);
        assert!(html.contains("<audio"), "{html}");
        assert!(html.contains("controls preload=\"metadata\""), "{html}");
        assert!(html.contains("data-ref=\"intro\""), "{html}");
        assert!(html.contains("data-title=\"Test Book\""), "{html}");
        assert!(html.contains("data-chapters="), "carries the chapter JSON: {html}");
        assert!(html.contains("&quot;start_ms&quot;"), "chapter JSON is attr-escaped: {html}");
        assert!(html.contains("data-artwork=\"/media/file/artkey\""), "{html}");
        assert!(
            html.contains("<source src=\"/media/file/fullaudio\" type=\"audio/mp4\">"),
            "{html}"
        );
        assert!(
            !html.contains("<source src=\"/media/file/artkey\""),
            "artwork is never a playback source: {html}"
        );
        assert!(
            html.contains("<img") && html.contains("src=\"/media/file/artkey\""),
            "the cover art renders as the visible header image: {html}"
        );
        assert!(
            html.contains(">Test Book</span>"),
            "the title renders as visible header text: {html}"
        );
        assert!(
            !html.contains("media-download"),
            "audio carries NO download button (DG.5): {html}"
        );
        assert!(html.contains("Your browser can't play this audio."), "{html}");
    }

    /// A chapterless, artless mp3 renders the bare player contract — no
    /// data-chapters, no artwork attr, no cover img — title header only.
    #[test]
    fn chapterless_audio_omits_optional_attrs() {
        let html = render_embed_html(&media("audio"), &[variant("mp3key", "audio/mpeg", None)]);
        assert!(html.contains("<audio"), "{html}");
        assert!(!html.contains("data-chapters"), "{html}");
        assert!(!html.contains("data-artwork"), "{html}");
        assert!(!html.contains("<img"), "no cover art → no header image: {html}");
        assert!(html.contains("type=\"audio/mpeg\""), "{html}");
    }

    #[test]
    fn file_renders_download_button_with_size() {
        let mut m = media("file");
        m.title = Some("Bracket.3mf".to_string());
        let mut v = variant("filekey", "model/3mf", None);
        v.bytes = 2_517_000; // ~2.4 MB
        let html = render_embed_html(&m, &[v]);
        assert!(html.contains("href=\"/media/file/filekey\""), "{html}");
        assert!(html.contains("download=\"Bracket.3mf\""), "download attr forces save: {html}");
        assert!(html.contains("Download Bracket.3mf"), "labelled with the name: {html}");
        assert!(html.contains("(2.4 MB)"), "shows a human size: {html}");
        assert!(html.contains("<svg"), "carries the download glyph: {html}");
    }

    #[test]
    fn human_bytes_scales_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2 KB");
        assert_eq!(human_bytes(2_517_000), "2.4 MB");
        assert_eq!(human_bytes(5_368_709_120), "5.0 GB");
    }
}
