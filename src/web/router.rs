use super::{
    app_state::AppState,
    features::{
        contact::contact,
        login::{
            authentication_options, finish_authentication, finish_registration, login_page, logout,
            start_registration,
        },
        projects::projects,
        resume::resume,
    },
    static_content::static_content,
};
use anyhow::Result;
use axum::{
    http::Uri,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use build_time::build_time_utc;
use reqwest::StatusCode;
use time::Duration;
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, TraceLayer},
};
use tower_sessions::{cookie::Key, Expiry, SessionManagerLayer};
use tracing::Level;

pub const BUILD_TIME_CACHE_BUST: &str = build_time_utc!("%s");

pub async fn create_router(app_state: AppState) -> Result<Router> {
    // Generate a cryptographic key to sign the session cookie.
    let key = Key::generate();

    let session_layer = SessionManagerLayer::new(app_state.session_store.clone())
        .with_secure(true)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)))
        .with_signed(key);

    Ok(Router::new()
        .route("/", get(projects))
        .route("/contact", get(contact))
        .route("/login", get(login_page))
        .route("/login/getAuthOptions", get(authentication_options))
        .route("/login/finish_authentication", post(finish_authentication))
        .route("/login/register/{display_name}", get(start_registration))
        .route("/login/finish_register", post(finish_registration))
        .route("/login/logout", get(logout))
        .route("/projects", get(projects))
        .route("/resume", get(resume))
        .with_state(app_state)
        .merge(static_content())
        .fallback(fallback)
        .layer(
            ServiceBuilder::new()
                .layer(
                    TraceLayer::new_for_http()
                        .make_span_with(DefaultMakeSpan::new().include_headers(true))
                        .on_request(DefaultOnRequest::new().level(Level::DEBUG))
                        .on_response(()),
                )
                .layer(session_layer)
                .layer(CompressionLayer::new()),
        ))
}

//TDOO: We should make our 404s fancy
async fn fallback(uri: Uri) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("No route for {uri}"))
}

// Example used to wrap our errors sanely: https://github.com/tokio-rs/axum/blob/main/examples/anyhow-error-response/src/main.rs

// Make our own error that wraps `anyhow::Error`.
pub struct AppError(anyhow::Error);

//TODO: This is not a secure approach, we should log and then give minimal information
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
