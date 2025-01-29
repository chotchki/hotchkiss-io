use crate::db::dao::crypto_key::get_or_create;

use super::{
    app_state::AppState,
    features::{contact::contact, login::login_router, projects::projects, resume::resume},
    static_content::static_content,
};
use anyhow::Result;
use axum::{http::Uri, routing::get, Router};
use build_time::build_time_utc;
use reqwest::StatusCode;
use time::Duration;
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, TraceLayer},
};
use tower_sessions::{Expiry, SessionManagerLayer};
use tracing::Level;

pub const BUILD_TIME_CACHE_BUST: &str = build_time_utc!("%s");

pub async fn create_router(app_state: AppState) -> Result<Router> {
    // Generate a cryptographic key to sign the session cookie.
    let key = get_or_create(&app_state.pool, 1).await?;

    let session_layer = SessionManagerLayer::new(app_state.session_store.clone())
        .with_secure(true)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)))
        .with_signed(key.key_value);

    Ok(Router::new()
        .route("/", get(projects))
        .route("/contact", get(contact))
        .route("/projects", get(projects))
        .route("/resume", get(resume))
        .nest("/login", login_router())
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
