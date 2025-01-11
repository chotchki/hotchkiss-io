use super::static_content::static_content;
use anyhow::Result;
use askama::Template;
use axum::{
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use build_time::build_time_utc;
use reqwest::StatusCode;
use sqlx::SqlitePool;
use time::Duration;
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, TraceLayer},
};
use tower_sessions::{
    cookie::Key,
    session_store::{self, ExpiredDeletion},
    Expiry, SessionManagerLayer,
};
use tower_sessions_sqlx_store::SqliteStore;
use tracing::Level;

pub const BUILD_TIME_CACHE_BUST: &str = build_time_utc!("%s");

pub async fn create_router(session_store: SqliteStore) -> Result<Router> {
    // Generate a cryptographic key to sign the session cookie.
    let key = Key::generate();

    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(true)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)))
        .with_signed(key);

    Ok(Router::new()
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
        ))
}

#[derive(Clone, Copy, Debug)]
pub enum NavSetting {
    Contact,
    Projects,
    Resume,
}

#[derive(Template)]
#[template(path = "contact.html")]
struct ContactTemplate {
    nav: NavSetting,
}

async fn contact() -> impl IntoResponse {
    let template = ContactTemplate {
        nav: NavSetting::Contact,
    };

    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "projects.html")]
struct ProjectsTemplate {
    nav: NavSetting,
}

async fn projects() -> impl IntoResponse {
    let template = ProjectsTemplate {
        nav: NavSetting::Projects,
    };
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "resume.html")]
struct ResumeTemplate {
    nav: NavSetting,
}

async fn resume() -> impl IntoResponse {
    let template = ResumeTemplate {
        nav: NavSetting::Resume,
    };
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
