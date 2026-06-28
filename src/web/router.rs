use super::{app_state::AppState, features::login::login_router, static_content::static_content};
use crate::{
    db::dao::crypto_key::CryptoKey,
    web::{
        features::{
            admin::admin_router,
            blog::blog_router,
            pages::{pages_router, projects::projects_router, redirect_to_first_page},
        },
        middleware::{
            api_key_auth::api_key_auth, request_log::log_requests,
            require_admin_for_mutations::require_admin_for_mutations,
        },
    },
};
use axum::{routing::get, Router};
use build_time::build_time_utc;
use time::Duration;
use tower::ServiceBuilder;
use tower_http::{
    compression::{
        predicate::{DefaultPredicate, NotForContentType, Predicate},
        CompressionLayer,
    },
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
    // API-key middleware needs the full state (the pool) — clone before app_state
    // is moved into `.with_state`.
    let api_key_state = app_state.clone();
    let router = Router::new()
        .route("/", get(redirect_to_first_page))
        .nest("/login", login_router())
        .nest("/pages", pages_router())
        .nest("/projects", projects_router())
        .nest("/blog", blog_router())
        .nest("/admin", admin_router())
        // Public media (Phase BZ): byte serve route + the embed swap target.
        .nest("/media", crate::web::features::media::media_router())
        // HTMX swap target for inline diagrams (public; renders only
        // server-registered sources — see web/features/diagram.rs).
        .route(
            "/diagram/{hash}",
            get(crate::web::features::diagram::render_registered_diagram),
        )
        // /resume + /resume.pdf (the latter generated via weasyprint) — top-level.
        .merge(crate::web::features::resume::resume_routes())
        // Unified Atom feed (blog posts + project pages). `/blog/feed.xml` serves
        // the same handler for back-compat (see blog_router).
        .route("/feed.xml", get(crate::web::features::feed::show_feed))
        // SEO: dynamic sitemap + robots (host-correct Sitemap directive, beta
        // de-indexed) — see web/features/seo.rs.
        .route("/sitemap.xml", get(crate::web::features::seo::sitemap_xml))
        .route("/robots.txt", get(crate::web::features::seo::robots_txt));

    // Debug-only test-login seam (absent from release builds = prod).
    #[cfg(debug_assertions)]
    let router = router.nest(
        "/test",
        crate::web::features::test_login::test_router(),
    );

    // `.fallback` BEFORE `.with_state` so the handler can extract
    // `State<AppState>` (it needs the pool to build the 404's nav).
    let router = router
        .fallback(crate::web::features::not_found::fallback)
        .with_state(app_state)
        .merge(static_content());

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
            // API-key auth (Phase CA): resolve `Authorization: Bearer hio_…` and
            // inject an Authenticated SessionData. OUTER to the authz layer so the
            // injection is present when SessionData is read; a key thus delegates
            // its user's role with no cookie.
            .layer(axum::middleware::from_fn_with_state(
                api_key_state,
                api_key_auth,
            ))
            // Fail-closed authz (Phase E): GET/HEAD/OPTIONS public; every other
            // method requires admin by default (except the anonymous auth
            // ceremony). INNER to session_layer so SessionData is populated.
            .layer(axum::middleware::from_fn(require_admin_for_mutations))
            // Compress text, NEVER binary media: gzipping a `video/*` (or other
            // already-compressed) `206` range response corrupts the byte ranges
            // the browser seeks/streams with — it played back jerky. Images are
            // already excluded by DefaultPredicate; add video/audio/model/binary.
            .layer(
                CompressionLayer::new().compress_when(
                    DefaultPredicate::new()
                        .and(NotForContentType::const_new("video/"))
                        .and(NotForContentType::const_new("audio/"))
                        .and(NotForContentType::const_new("model/"))
                        .and(NotForContentType::const_new("application/octet-stream")),
                ),
            ),
    );

    Ok(router)
}
