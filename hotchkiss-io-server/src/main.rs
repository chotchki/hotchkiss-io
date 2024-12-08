/// Built off this tutorial: https://joeymckenzie.tech/blog/templates-with-rust-axum-htmx-askama
use anyhow::Context;
use askama::Template;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use coordinator::{ip::omada_config::OmadaConfig, Coordinator};
use hotchkiss_io_db::DatabaseHandle;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io, net::IpAddr};
use tokio::net::lookup_host;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod certificate;
mod coordinator;

#[derive(Serialize, Deserialize)]
struct Settings {
    pub cloudflare_token: String,
    pub database_path: String,
    pub domain: String,
    pub omada_config: OmadaConfig,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hotchkiss_io=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    //Read our sensitive data from stdin
    let stdin = io::read_to_string(io::stdin())?;
    let settings: Settings = serde_json::from_str(&stdin)?;

    //Build the coordinator
    let mut coordinator = Coordinator::create(settings).await?;
    coordinator.start().await?; //This never returns

    info!("initializing router...");

    let router = Router::new().route("/", get(hello));
    let port = 80_u16;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));

    info!("router initialized, now listening on port {}", port);

    axum_server::bind(addr)
        .serve(router.into_make_service())
        .await
        .context("error while starting server")?;

    Ok(())
}

async fn hello() -> impl IntoResponse {
    let template = HelloTemplate {};
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate;

/// A wrapper type that we'll use to encapsulate HTML parsed by askama into valid HTML for axum to serve.
struct HtmlTemplate<T>(T);

/// Allows us to convert Askama HTML templates into valid HTML for axum to serve in the response.
impl<T> IntoResponse for HtmlTemplate<T>
where
    T: Template,
{
    fn into_response(self) -> Response {
        // Attempt to render the template with askama
        match self.0.render() {
            // If we're able to successfully parse and aggregate the template, serve it
            Ok(html) => Html(html).into_response(),
            // If we're not, return an error or some bit of fallback HTML
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template. Error: {}", err),
            )
                .into_response(),
        }
    }
}
