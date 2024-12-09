use axum::{
    body::{boxed, Full},
    handler::Handler,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::RustEmbed;

use crate::GIT_VERSION;

pub fn router() -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/favicon.ico", static_handler.into_service())
        .route("/index.html", get(index_handler))
        .route("/manifest.json", static_handler.into_service())
        .route("/robots.txt", static_handler.into_service())
        .route("/icons/*file", static_handler.into_service())
        .route("/static/*file", static_handler.into_service())
}

async fn index_handler() -> impl IntoResponse {
    static_handler("/index.html".parse::<Uri>().unwrap()).await
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/').to_string();

    StaticFile(path)
}

// Static Example from here: https://github.com/pyrossh/rust-embed/blob/master/examples/axum.rs
#[derive(RustEmbed)]
#[folder = "$FRONTEND_BUILD_DIR"]
struct Asset;

pub struct StaticFile<T>(pub T);

impl<T> IntoResponse for StaticFile<T>
where
    T: Into<String>,
{
    fn into_response(self) -> Response {
        let path = self.0.into();

        tracing::debug!("Got static content request for {}", path);
        for file in Asset::iter() {
            tracing::debug!("File known {}", file.as_ref());
        }

        match Asset::get(path.as_str()) {
            Some(content) => {
                let body = boxed(Full::from(content.data));
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                Response::builder()
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .header("x-git-version", GIT_VERSION)
                    .body(body)
                    .unwrap()
            }
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(boxed(Full::from(
                    "404 - Yeah you're statically not finding what you want.",
                )))
                .unwrap(),
        }
    }
}
