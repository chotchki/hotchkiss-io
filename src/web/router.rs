use super::{app_state::AppState, features::login::login_router, static_content::static_content};
use crate::{
    db::dao::crypto_key::CryptoKey,
    web::{
        features::{
            admin::admin_router,
            blog::blog_router,
            pages::{
                attachments::attachments_router, pages_router, projects::projects_router,
                redirect_to_first_page,
            },
        },
        middleware::request_log::log_requests,
    },
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
    // `Secure` cookies aren't sent over plain HTTP, which the local test harness
    // / dev server use — so only set it in release (= prod, which is HTTPS-only).
    let session_layer = SessionManagerLayer::new(app_state.session_store.clone())
        .with_secure(!cfg!(debug_assertions))
        .with_expiry(Expiry::OnInactivity(Duration::days(1)))
        .with_signed(key.key()?);

    debug!("Making router");
    let log_pool = app_state.pool.clone();
    let router = Router::new()
        .route("/", get(redirect_to_first_page))
        .nest("/login", login_router())
        .nest("/attachments", attachments_router())
        .nest("/pages", pages_router())
        .nest("/projects", projects_router())
        .nest("/blog", blog_router())
        .nest("/admin", admin_router())
        // HTMX swap target for inline diagrams (public; renders only
        // server-registered sources — see web/features/diagram.rs).
        .route(
            "/diagram/{hash}",
            get(crate::web::features::diagram::render_registered_diagram),
        );

    // Debug-only test-login seam (absent from release builds = prod).
    #[cfg(debug_assertions)]
    let router = router.nest(
        "/test",
        crate::web::features::test_login::test_router(),
    );

    let router = router
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
            // Outermost: see every request + the final response status.
            .layer(axum::middleware::from_fn_with_state(log_pool, log_requests))
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
