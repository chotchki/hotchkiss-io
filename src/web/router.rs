use super::static_content::static_content;
use askama::Template;
use axum::{
    body::Bytes,
    http::Request,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use http_body_util::Full;
use reqwest::StatusCode;
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::{Level, Span};

pub fn create_router() -> Router {
    Router::new()
        .route("/", get(projects))
        .route("/contact", get(contact))
        .route("/projects", get(projects))
        .route("/resume", get(resume))
        .merge(static_content())
        .layer(
            ServiceBuilder::new()
                .layer(
                    TraceLayer::new_for_http()
                        .make_span_with(DefaultMakeSpan::new().include_headers(true))
                        .on_request(DefaultOnRequest::new().level(Level::DEBUG))
                        .on_response(()),
                )
                .layer(CompressionLayer::new()),
        )
}

async fn contact() -> impl IntoResponse {
    let template = ContactTemplate {};
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "contact.html")]
struct ContactTemplate;

async fn projects() -> impl IntoResponse {
    let template = ProjectsTemplate {};
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "projects.html")]
struct ProjectsTemplate;

#[derive(Template)]
#[template(path = "resume.html")]
struct ResumeTemplate;

async fn resume() -> impl IntoResponse {
    let template = ResumeTemplate {};
    HtmlTemplate(template)
}

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
