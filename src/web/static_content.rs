use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use build_time::build_time_utc;
use rust_embed::RustEmbed;
use tracing::Level;

use crate::web::app_state::AppState;
use crate::web::util::host::{is_canonical_host, request_host};

const BUILD_TIME: &str = build_time_utc!("%a, %d %b %Y %H:%M:%S GMT");

pub fn static_content() -> Router<AppState> {
    Router::new()
        // Browsers request /favicon.ico (and iOS /apple-touch-icon.png) at the ROOT by
        // default — the tab icon before HTML parses, bookmarks, non-HTML contexts —
        // regardless of the <link rel=icon>. Serve the images/ assets there too, else
        // every visitor 404s the icon (523 favicon hits surfaced in the CR analytics
        // "only ever errored" list). The old commented route wouldn't have worked: it
        // maps the URL path to the asset path, and the icon lives under images/.
        .route("/favicon.ico", get(favicon))
        .route("/apple-touch-icon.png", get(apple_touch_icon))
        // Legacy iOS probes the -precomposed variant at root too; serve the same icon.
        .route("/apple-touch-icon-precomposed.png", get(apple_touch_icon))
        .route("/images/{*file}", get(static_handler))
        .route("/manifest.webmanifest", get(static_handler))
        // /robots.txt is served dynamically (host-aware Sitemap directive + beta
        // de-index) by web/features/seo.rs, NOT from the static asset.
        .route("/scripts/{*file}", get(static_handler))
        .route("/styles/{*file}", get(static_handler))
        .route("/vendor/{*file}", get(static_handler))
}

async fn static_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/').to_string();
    StaticFile {
        // A query string means a cache-busting request (every asset is linked as
        // `?cb=<build epoch>`), so the URL changes whenever the bytes do.
        versioned: uri.query().is_some(),
        path: icon_path_for_host(&path, canonical(&state, &headers, &uri)),
    }
}

/// EB.8: the two identity icons swap to their INVERTED `-beta` variants on a
/// non-canonical host, so the beta PWA pin is visually distinct from prod on the
/// home screen. Everything else passes through untouched. Covers the base.html
/// `<link>` paths AND the root fallback routes (both funnel through here).
fn icon_path_for_host(path: &str, canonical: bool) -> String {
    if canonical {
        return path.to_string();
    }
    match path {
        "images/favicon.ico" => "images/favicon-beta.ico".to_string(),
        "images/apple-touch-icon.png" => "images/apple-touch-icon-beta.png".to_string(),
        _ => path.to_string(),
    }
}

fn canonical(state: &AppState, headers: &HeaderMap, uri: &Uri) -> bool {
    is_canonical_host(&request_host(headers, uri), &state.site_host)
}

/// Root `/favicon.ico` → the embedded `images/favicon.ico` (host-aware, EB.8).
/// Un-versioned (the root URL carries no `?cb=`), so it gets the modest 1-day
/// TTL, not immutable.
async fn favicon(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    StaticFile {
        path: icon_path_for_host("images/favicon.ico", canonical(&state, &headers, &uri)),
        versioned: false,
    }
}

/// Root `/apple-touch-icon.png` → the embedded `images/apple-touch-icon.png` (iOS
/// requests it at the root by default; host-aware, EB.8).
async fn apple_touch_icon(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    StaticFile {
        path: icon_path_for_host(
            "images/apple-touch-icon.png",
            canonical(&state, &headers, &uri),
        ),
        versioned: false,
    }
}

// Static Example from here: https://github.com/pyrossh/rust-embed/blob/master/examples/axum.rs
#[derive(RustEmbed)]
#[folder = "assets"]
struct Asset;

pub struct StaticFile {
    pub path: String,
    /// The request carried a cache-busting query (`?cb=…`). Because that token is
    /// the build epoch, a stale cache can never serve the wrong bytes — so a
    /// versioned hit is safe to cache `immutable` for a year.
    pub versioned: bool,
}

impl IntoResponse for StaticFile {
    fn into_response(self) -> Response {
        let StaticFile { path, versioned } = self;

        tracing::debug!("Got static content request for {}", path);
        if tracing::enabled!(Level::TRACE) {
            for file in Asset::iter() {
                tracing::trace!("File known {}", file.as_ref());
            }
        }

        match Asset::get(path.as_str()) {
            Some(content) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                let debug = cfg!(debug_assertions);
                let mut rb = Response::builder()
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .header(header::CACHE_CONTROL, cache_control(debug, versioned));
                // LAST_MODIFIED only helps the revalidatable case (un-versioned,
                // release); immutable + no-store never revalidate.
                if !debug && !versioned {
                    rb = rb.header(header::LAST_MODIFIED, BUILD_TIME);
                }
                rb.body(Body::from(content.data)).unwrap()
            }
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(
                    "404 - Yeah you're statically not finding what you want.",
                ))
                .unwrap(),
        }
    }
}

/// `Cache-Control` for a static asset (Phase CN). Versioned (`?cb=`) requests are
/// immutable for a year — the URL carries the build epoch, so a stale cache can
/// never serve the wrong bytes; un-versioned bare fetches get a modest TTL; debug
/// never caches (live-reload). Split out as a pure fn so the release policy is
/// unit-testable from the debug test harness.
fn cache_control(debug: bool, versioned: bool) -> &'static str {
    match (debug, versioned) {
        (true, _) => "no-store",
        (false, true) => "public, max-age=31536000, immutable",
        (false, false) => "max-age=86400",
    }
}

#[cfg(test)]
mod tests {
    use super::{cache_control, icon_path_for_host, Asset};

    #[test]
    fn beta_icon_swap() {
        // Non-canonical host (beta) swaps ONLY the two identity icons.
        assert_eq!(
            icon_path_for_host("images/favicon.ico", false),
            "images/favicon-beta.ico"
        );
        assert_eq!(
            icon_path_for_host("images/apple-touch-icon.png", false),
            "images/apple-touch-icon-beta.png"
        );
        assert_eq!(
            icon_path_for_host("images/icon-512.png", false),
            "images/icon-512.png"
        );
        // Canonical host: untouched.
        assert_eq!(
            icon_path_for_host("images/favicon.ico", true),
            "images/favicon.ico"
        );
        // The inverted assets must actually be embedded — a rename/removal would
        // otherwise silently 404 beta's icon.
        assert!(Asset::get("images/favicon-beta.ico").is_some());
        assert!(Asset::get("images/apple-touch-icon-beta.png").is_some());
    }

    #[test]
    fn cache_policy() {
        // debug never caches, regardless of versioning (live-reload).
        assert_eq!(cache_control(true, true), "no-store");
        assert_eq!(cache_control(true, false), "no-store");
        // release: a versioned (?cb=) hit caches a year immutable; a bare hit a day.
        assert_eq!(
            cache_control(false, true),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control(false, false), "max-age=86400");
    }
}
