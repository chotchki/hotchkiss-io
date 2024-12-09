//Used from here: https://github.com/tokio-rs/axum/blob/main/examples/reverse-proxy/src/main.rs
use axum::{
    body::Body,
    extract::{Request, State},
    http::uri::Uri,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use hyper_util::{client::legacy::connect::HttpConnector, rt::TokioExecutor};
use reqwest::StatusCode;

type Client = hyper_util::client::legacy::Client<HttpConnector, Body>;

const NPM_SERVICE: &str = "http://localhost:3000";

pub fn router() -> Router {
    let client: Client =
        hyper_util::client::legacy::Client::<(), ()>::builder(TokioExecutor::new())
            .build(HttpConnector::new());

    Router::new()
        .route("/", get(index_handler))
        .route("/favicon.ico", get(handler))
        .route("/index.html", get(index_handler))
        .route("/manifest.json", get(handler))
        .route("/robots.txt", get(handler))
        .route("/icons/*file", get(handler))
        .route("/static/*file", get(handler))
        .with_state(client)
}

async fn index_handler(
    State(client): State<Client>,
    // NOTE: Make sure to put the request extractor last because once the request
    // is extracted, extensions can't be extracted anymore.
    mut req: Request<Body>,
) -> Result<Response, StatusCode> {
    let uri = format!("{}{}", NPM_SERVICE, "/index.html");
    tracing::debug!("Got static content request for / requesting {}", uri);

    *req.uri_mut() = Uri::try_from(uri).unwrap();

    Ok(client
        .request(req)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .into_response())
}

async fn handler(
    State(client): State<Client>,
    // NOTE: Make sure to put the request extractor last because once the request
    // is extracted, extensions can't be extracted anymore.
    mut req: Request<Body>,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();

    let path_query = req
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(path);

    let uri = format!("{}{}", NPM_SERVICE, path_query);
    tracing::debug!("Got static content request for {} requesting {}", path, uri);

    *req.uri_mut() = Uri::try_from(uri).unwrap();

    Ok(client
        .request(req)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .into_response())
}
