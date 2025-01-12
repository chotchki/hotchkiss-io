use super::{
    features::{contact::contact, login::loginPage, projects::projects, resume::resume},
    static_content::static_content,
};
use anyhow::Result;
use axum::{routing::get, Router};
use build_time::build_time_utc;
use time::Duration;
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, TraceLayer},
};
use tower_sessions::{cookie::Key, Expiry, SessionManagerLayer};
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
        .route("/login", get(loginPage))
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
