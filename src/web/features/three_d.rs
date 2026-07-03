//! `/3d` — the 3D-printing gallery index (Phase CW, gallery half).
//!
//! Lists the children of the `3d` special page as model cards: a **Featured** band
//! (the pinned showpieces — the SAME Pin button / `featured` tag the landing uses,
//! but scoped here) above the rest. Model detail pages live under the content tree
//! at `/pages/3d/<slug>` and are served by the ordinary `get_page_path`, so this
//! module owns only the index. 3D never appears on `/` — `show_home` only fetches
//! `blog` + `projects`, so a `featured`-tagged model surfaces ONLY here.
//!
//! Later this root hosts the WASM slicer/placer editor (CW.1–4); the nesting is
//! unchanged when it lands.

use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate,
        markdown::render_cache::cached_excerpt, session::SessionData,
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::RustEmbed;

pub fn three_d_router() -> Router<AppState> {
    Router::new()
        .route("/", get(show_3d_index))
        // The WASM slicer/placer editor (Phase CW) on its OWN route, so the
        // COOP/COEP cross-origin isolation stays off the rest of the site.
        .route("/editor", get(editor_document))
        .route("/editor/fab_web.js", get(editor_js))
        .route("/editor/fab_web_bg.wasm", get(editor_wasm))
}

/// A model card for the `/3d` gallery — cover render, title, excerpt — linking to
/// the model's detail page at `/pages/3d/<slug>`. Mirrors the project card.
pub struct ModelCard {
    pub page_name: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub excerpt: String,
    /// Future-dated (scheduled) — admin-only, drives the "Scheduled" badge (CU).
    pub is_scheduled: bool,
}

#[derive(Template)]
#[template(path = "3d/index.html")]
pub struct ThreeDIndexTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    /// Pinned showpieces (the `featured` tag), `page_order`-sorted.
    pub featured: Vec<ModelCard>,
    /// The rest of the (published) models, in manual `page_order`.
    pub models: Vec<ModelCard>,
    pub meta: crate::web::features::seo::Meta,
}

async fn card_from(state: &AppState, page: &ContentPageDao) -> ModelCard {
    ModelCard {
        title: page.display_title(),
        page_name: page.page_name.clone(),
        cover_url: crate::web::features::media::cover_url_for(&state.pool, page.page_id).await,
        excerpt: cached_excerpt(&page.page_markdown),
        is_scheduled: page.is_scheduled(),
    }
}

pub async fn show_3d_index(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let three_d = ContentPageDao::find_by_name(&state.pool, None, "3d").await?;
    let Some(three_d) = three_d else {
        return Err(anyhow!("Server misconfiguration, could not find the `3d` special page").into());
    };

    // Children in manual page_order (drag-reorder like /projects).
    let mut rows = ContentPageDao::find_by_parent(&state.pool, Some(three_d.page_id)).await?;
    // Scheduled/timed publishing gate (CU): hide future-dated models from non-admins.
    let is_admin = session_data.auth_state.is_admin();
    rows.retain(|p| p.is_visible_to(is_admin));

    // Pinned → Featured (page_order-sorted, recency-tiebroken like the landing);
    // the rest below. Reuses the exact Pin/`featured` mechanism, scoped to 3D.
    let (mut featured_rows, rest): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|p| p.is_featured());
    featured_rows.sort_by(|a, b| {
        a.page_order
            .cmp(&b.page_order)
            .then(b.page_creation_date.cmp(&a.page_creation_date))
            .then(b.page_id.cmp(&a.page_id))
    });

    let mut featured: Vec<ModelCard> = Vec::with_capacity(featured_rows.len());
    for p in &featured_rows {
        featured.push(card_from(&state, p).await);
    }
    let mut models: Vec<ModelCard> = Vec::with_capacity(rest.len());
    for p in &rest {
        models.push(card_from(&state, p).await);
    }

    let meta = crate::web::features::seo::Meta::section(
        &state.site_host,
        "3D — Christopher Hotchkiss".to_string(),
        "3D-printed hardware and OpenSCAD designs by Christopher Hotchkiss — the physical half of the portfolio.".to_string(),
        "3d",
    );

    let template = ThreeDIndexTemplate {
        top_bar: TopBar::create(&state.pool, "3d").await?,
        auth_state: session_data.auth_state,
        featured,
        models,
        meta,
    };
    Ok(HtmlTemplate(template).into_response())
}

// ── The WASM slicer/placer editor (Phase CW) ──────────────────────────────────

/// The fab-web WASM bundle, embedded from `$OUT_DIR/fab-web` — build.rs downloads +
/// sha256-verifies the pinned release and drops the raw 32 MB wasm, so only the
/// brotli/gzip variants + the JS glue ride in the binary.
#[derive(RustEmbed)]
#[folder = "$OUT_DIR/fab-web"]
struct FabWeb;

/// The editor's top-level document — the site OWNS it (the bundle ships an
/// `index.reference.html` to crib from). Absolute import path so the glue's
/// `import.meta.url` resolves the wasm under `/3d/editor/`. The `init().catch`
/// swallows winit's "control flow" exit exception (it would otherwise read as a
/// crash). COOP/COEP live on THIS document (per the contract — future wasm-threads
/// headroom; the v1 bundle is single-threaded and runs without them).
const EDITOR_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>fab — 3D slicer/placer · Christopher Hotchkiss</title>
<style>
  html,body{margin:0;height:100%;background:#1a2b4a;overflow:hidden}
  canvas{display:block}
  #back{position:fixed;top:.5rem;left:.5rem;z-index:10;background:#1a2b4a;color:#ffc935;
        font:600 .8rem system-ui,sans-serif;padding:.35rem .6rem;border-radius:.3rem;text-decoration:none}
  #back:hover{background:#24365a}
</style>
</head>
<body>
<a id="back" href="/3d">&larr; 3D</a>
<script type="module">
  import init from '/3d/editor/fab_web.js';
  init().catch(e => {
    if (!`${e}`.includes('Using exceptions for control flow')) console.error('INIT ERROR:', e);
  });
</script>
</body>
</html>
"#;

/// COOP+COEP-carrying editor document. Isolation is scoped to `/3d/editor*`; the
/// rest of the site (content pages, media embeds, the model gallery) is never
/// cross-origin isolated.
async fn editor_document() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header("cross-origin-opener-policy", "same-origin")
        .header("cross-origin-embedder-policy", "require-corp")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(EDITOR_HTML))
        .expect("static editor document is a valid response")
}

/// The wasm-bindgen JS glue (ES module). Same-origin → loads under the document's
/// COEP `require-corp` with no CORP needed.
async fn editor_js() -> Response {
    match FabWeb::get("fab_web.js") {
        Some(f) => Response::builder()
            .header(header::CONTENT_TYPE, "text/javascript; charset=utf-8")
            .header(header::CACHE_CONTROL, "public, max-age=3600")
            .body(Body::from(f.data))
            .expect("embedded js is a valid response"),
        None => bundle_missing("fab_web.js"),
    }
}

/// The wasm, served PRECOMPRESSED (brotli if accepted, gzip otherwise) with
/// `Content-Type: application/wasm` — the glue uses `instantiateStreaming`, so the
/// MIME must be exact. Already carrying `Content-Encoding`, it skips the site's
/// on-the-fly CompressionLayer (never double-compressed). The raw wasm isn't
/// embedded, so a client here must accept br or gz (every browser does).
async fn editor_wasm(headers: HeaderMap) -> Response {
    let (asset, encoding) = if accepts_encoding(&headers, "br") {
        ("fab_web_bg.wasm.br", "br")
    } else {
        ("fab_web_bg.wasm.gz", "gzip")
    };
    match FabWeb::get(asset) {
        Some(f) => Response::builder()
            .header(header::CONTENT_TYPE, "application/wasm")
            .header(header::CONTENT_ENCODING, encoding)
            .header(header::CACHE_CONTROL, "public, max-age=3600")
            .body(Body::from(f.data))
            .expect("embedded wasm is a valid response"),
        None => bundle_missing(asset),
    }
}

/// Does the client's `Accept-Encoding` list `enc`?
fn accepts_encoding(headers: &HeaderMap, enc: &str) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .any(|e| e.trim().split(';').next().unwrap_or("").trim() == enc)
        })
        .unwrap_or(false)
}

/// A missing bundle file is a build/config error, not a user-fixable 404 — surface
/// it loudly (500) so a broken embed is obvious.
fn bundle_missing(name: &str) -> Response {
    tracing::error!("fab-web bundle file missing from the embed: {name}");
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from(format!("fab-web bundle incomplete: {name}")))
        .expect("error response is valid")
}
