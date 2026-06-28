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

use axum::extract::{Path, Request, State};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use http::{header, HeaderValue, StatusCode};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::db::dao::media::{MediaDao, MediaKind, MediaVariantDao};
use crate::media::is_sha256_hex;
use crate::web::app_state::AppState;

/// In-flow image height cap (matches the markdown transformer's content images).
const MAX_IMAGE_HEIGHT_PX: u32 = 480;

pub fn media_router() -> Router<AppState> {
    Router::new()
        .route("/file/{url_key}", get(serve_media_file))
        .route("/embed/{media_ref}", get(render_media_embed))
}

/// Stream the bytes for a variant, addressed by its public HMAC `url_key`. Range
/// requests are handled by `ServeFile` (206). Content is immutable → cache hard.
async fn serve_media_file(
    State(state): State<AppState>,
    Path(url_key): Path<String>,
    req: Request,
) -> Response {
    // The token is 64 lowercase hex (HMAC-SHA256) — gate the format so a junk
    // path can't reach the store, and a miss looks identical to a non-existent
    // file (no oracle).
    if !is_sha256_hex(&url_key) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let variant = match MediaVariantDao::find_by_url_key(&state.pool, &url_key).await {
        Ok(Some(v)) => v,
        Ok(None) => return (StatusCode::NOT_FOUND, "Not found").into_response(),
        Err(e) => {
            tracing::error!("media lookup by url_key failed: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "media lookup failed").into_response();
        }
    };
    let path = state.media_store.path_for(&variant.sha256);
    let mime: mime_guess::mime::Mime = variant
        .mime
        .parse()
        .unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM);

    match ServeFile::new_with_mime(&path, &mime).oneshot(req).await {
        Ok(r) => {
            let mut resp = r.into_response();
            resp.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
            resp
        }
        Err(e) => {
            tracing::error!("serving media file {path:?} failed: {e:?}");
            (StatusCode::INTERNAL_SERVER_ERROR, "serve failed").into_response()
        }
    }
}

/// HTMX swap target: resolve a media ref to its rendered element.
async fn render_media_embed(
    State(state): State<AppState>,
    Path(media_ref): Path<String>,
) -> Response {
    let media = match MediaDao::find_by_ref(&state.pool, &media_ref).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Html(error_span("media not found — the page may need a reload")).into_response()
        }
        Err(e) => {
            tracing::error!("media embed lookup failed: {e:?}");
            return Html(error_span("media lookup failed")).into_response();
        }
    };
    let variants = MediaVariantDao::find_by_media_id(&state.pool, media.media_id)
        .await
        .unwrap_or_default();
    Html(render_embed_html(&media, &variants)).into_response()
}

/// Build the element for a media item — the polymorphic dispatch on `kind`.
/// `pub(crate)` so the admin library can reuse the playable `<video>`.
pub(crate) fn render_embed_html(media: &MediaDao, variants: &[MediaVariantDao]) -> String {
    let alt = attr_escape(media.title.as_deref().unwrap_or(&media.media_ref));
    let kind = media.kind().unwrap_or(MediaKind::File);
    match kind {
        MediaKind::Image => {
            let Some(v) = variants.first() else {
                return error_span("image has no stored file");
            };
            format!(
                "<img class=\"content-image mx-auto my-4 block cursor-zoom-in\" \
style=\"max-width:100%;max-height:{MAX_IMAGE_HEIGHT_PX}px\" data-zoomable=\"true\" tabindex=\"0\" \
role=\"button\" aria-label=\"Zoom image\" src=\"/media/file/{}\" alt=\"{alt}\" />",
                v.url_key
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
                "<video class=\"media-video mx-auto my-4 block max-w-full h-auto rounded-md border-4 border-navy\" \
controls preload=\"metadata\" playsinline{poster}{dims}>{sources}\
Your browser can't play this video.</video>"
            )
        }
        MediaKind::Stl => {
            let Some(v) = variants.first() else {
                return error_span("stl has no stored file");
            };
            format!(
                "<object class=\"stl-view size-40 m-2 rounded-md border-8 border-navy\" data-filename=\"/media/file/{}\"></object>",
                v.url_key
            )
        }
        MediaKind::File => {
            let Some(v) = variants.first() else {
                return error_span("file has no content");
            };
            format!(
                "<a class=\"text-navy underline\" href=\"/media/file/{}\">{alt}</a>",
                v.url_key
            )
        }
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
    sqlx::query_scalar!(
        r#"SELECT v.url_key FROM content_pages c
           JOIN media_variant v ON v.media_id = c.page_cover_media_id
           WHERE c.page_id = ?1 AND v.mime LIKE 'image/%'
           ORDER BY v.variant_id LIMIT 1"#,
        page_id
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|k| format!("/media/file/{k}"))
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
    fn stl_renders_object_viewer() {
        let html = render_embed_html(&media("stl"), &[variant("stlkey", "model/stl", None)]);
        assert!(html.contains("<object class=\"stl-view"), "{html}");
        assert!(html.contains("data-filename=\"/media/file/stlkey\""), "{html}");
    }
}
