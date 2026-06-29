use axum::{
    Router,
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use build_time::build_time_utc;
use rust_embed::RustEmbed;
use tracing::Level;

const BUILD_TIME: &str = build_time_utc!("%a, %d %b %Y %H:%M:%S GMT");

pub fn static_content() -> Router {
    Router::new()
        //.route("/favicon.ico", get(static_handler))
        .route("/images/{*file}", get(static_handler))
        .route("/manifest.webmanifest", get(static_handler))
        // /robots.txt is served dynamically (host-aware Sitemap directive + beta
        // de-index) by web/features/seo.rs, NOT from the static asset.
        .route("/scripts/{*file}", get(static_handler))
        .route("/styles/{*file}", get(static_handler))
        .route("/vendor/{*file}", get(static_handler))
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    StaticFile {
        // A query string means a cache-busting request (every asset is linked as
        // `?cb=<build epoch>`), so the URL changes whenever the bytes do.
        versioned: uri.query().is_some(),
        path: uri.path().trim_start_matches('/').to_string(),
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
                let mut rb = Response::builder().header(header::CONTENT_TYPE, mime.as_ref());

                if cfg!(debug_assertions) {
                    rb = rb.header(header::CACHE_CONTROL, "no-store");
                } else if versioned {
                    // ?cb=<build epoch> changes every release → a year of immutable
                    // caching, and the LCP-critical CSS/JS/fonts stop re-validating.
                    rb = rb.header(header::CACHE_CONTROL, "public, max-age=31536000, immutable");
                } else {
                    // Un-versioned path (a bare favicon/manifest/apple-touch-icon
                    // fetch with no ?cb=): a modest TTL so an update lands within a day.
                    rb = rb
                        .header(header::LAST_MODIFIED, BUILD_TIME)
                        .header(header::CACHE_CONTROL, "max-age=86400");
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
