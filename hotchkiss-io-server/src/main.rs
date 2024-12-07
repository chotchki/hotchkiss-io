/// Built off this tutorial: https://joeymckenzie.tech/blog/templates-with-rust-axum-htmx-askama
use anyhow::Context;
use askama::Template;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use hotchkiss_io_ip::{OmadaClient, OmadaConfig};
use serde::{Deserialize, Serialize};
use std::io;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod certificate;

#[derive(Serialize, Deserialize)]
struct Settings {
    pub cloudflare_token: String,
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

    //Get our public ip so we can figure out certs
    let mut omada_client = OmadaClient::new(settings.omada_config)?;
    omada_client.login().await?;
    let public_ip = omada_client.get_wan_ip().await?;

    //Construct our data storage

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
