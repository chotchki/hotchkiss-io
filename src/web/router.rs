use super::{app_state::AppState, features::login::login_router, static_content::static_content};
use crate::{
    db::dao::crypto_key::CryptoKey,
    web::features::pages::{pages_router, redirect_to_first_page},
};
use axum::{http::Uri, routing::get, Router};
use build_time::build_time_utc;
use reqwest::StatusCode;
use time::Duration;
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, TraceLayer},
};
use tower_livereload::LiveReloadLayer;
use tower_sessions::{Expiry, SessionManagerLayer};
use tracing::{debug, Level};

pub const BUILD_TIME_CACHE_BUST: &str = build_time_utc!("%s");

pub async fn create_router(app_state: AppState) -> anyhow::Result<Router> {
    // Generate a cryptographic key to sign the session cookie.
    debug!("Getting cookie key");
    let key = CryptoKey::get_or_create(&app_state.pool, 1).await?;

    debug!("Making session layer");
    let session_layer = SessionManagerLayer::new(app_state.session_store.clone())
        .with_secure(true)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)))
        .with_signed(key.key_value);

    debug!("Making router");
    let router = Router::new()
        .route("/", get(redirect_to_first_page))
        .nest("/login", login_router())
        //.nest("/attachments", attachments_router())
        .nest("/pages", pages_router())
        .with_state(app_state)
        .merge(static_content())
        .fallback(fallback);

    let router = if cfg!(debug_assertions) {
        router.layer(LiveReloadLayer::new())
    } else {
        router
    };

    let router = router.layer(
        ServiceBuilder::new()
            .layer(
                TraceLayer::new_for_http()
                    .make_span_with(DefaultMakeSpan::new().include_headers(true))
                    .on_request(DefaultOnRequest::new().level(Level::DEBUG))
                    .on_response(()),
            )
            .layer(session_layer)
            .layer(CompressionLayer::new()),
    );

    Ok(router)
}

//TDOO: We should make our 404s fancy
async fn fallback(uri: Uri) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("No route for {uri}"))
}
