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
        .route("/robots.txt", get(static_handler))
        .route("/scripts/{*file}", get(static_handler))
        .route("/styles/{*file}", get(static_handler))
        .route("/vendor/{*file}", get(static_handler))
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/').to_string();

    StaticFile(path)
}

// Static Example from here: https://github.com/pyrossh/rust-embed/blob/master/examples/axum.rs
#[derive(RustEmbed)]
#[folder = "assets"]
struct Asset;

pub struct StaticFile<T>(pub T);

impl<T> IntoResponse for StaticFile<T>
where
    T: Into<String>,
{
    fn into_response(self) -> Response {
        let path = self.0.into();

        //tracing::debug!("Got static content request for {}", path);
        //if tracing::enabled!(Level::TRACE) {
        //    for file in Asset::iter() {
        //        tracing::trace!("File known {}", file.as_ref());
        //    }
        //}

        match Asset::get(path.as_str()) {
            Some(content) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                let mut rb = Response::builder().header(header::CONTENT_TYPE, mime.as_ref());

                if cfg!(debug_assertions) {
                    rb = rb.header(header::CACHE_CONTROL, "no-store");
                } else {
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
