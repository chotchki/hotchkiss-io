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
    extract::{Path, State},
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
        // COOP/COEP cross-origin isolation stays off the rest of the site. The
        // resource URLs are VERSION-PATHED (`/editor/<ver>/…`) so a bundle bump
        // changes the URL (cache-bust); the glue resolves the wasm relative to its
        // own path, so the version rides through to fab_gui_bg.wasm — both
        // immutable within a version, and never version-skewed. `{_v}` is ignored
        // (the embed is always the current version; the document only ever links
        // the current path).
        .route("/editor", get(editor_document))
        // ALL bundle files under the versioned prefix. The fab-gui bundle is a TREE
        // (fab_gui.js/.wasm PLUS a geom/ Manifold-kernel worker + wasm, and a root
        // libs.json the app fetches once); the glue fetches them relative to its own
        // path, so one wildcard serves the lot.
        .route("/editor/{_v}/{*path}", get(editor_asset))
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
    /// The min_role gate's badge label (from the fail-closed decode; None =
    /// public, no badge) — renders beside the Scheduled pill.
    pub visibility: Option<&'static str>,
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
        visibility: page.visibility_label(),
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

    // Section gate (DA): a min_role on the `3d` special row darkens the code
    // route too — same cat-404 as a genuine miss (see blog::show_index).
    let viewer = session_data.auth_state.role();
    if !three_d.is_visible_to(viewer) {
        return Ok(crate::web::features::not_found::render_not_found(
            &state.pool,
            session_data.auth_state,
        )
        .await);
    }

    // Children in manual page_order (drag-reorder like /projects).
    let mut rows = ContentPageDao::find_by_parent(&state.pool, Some(three_d.page_id)).await?;
    // Visibility gate (CU scheduling + DA min_role): hide future-dated or
    // role-gated models from insufficient viewers.
    rows.retain(|p| p.is_visible_to(viewer));

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
        top_bar: TopBar::create(&state.pool, "3d", viewer).await?,
        auth_state: session_data.auth_state,
        featured,
        models,
        meta,
    };
    Ok(HtmlTemplate(template).into_response())
}

// ── The WASM slicer/placer editor (Phase CW) ──────────────────────────────────

/// The fab-gui WASM bundle, embedded from `$OUT_DIR/fab-gui` — build.rs downloads +
/// sha256-verifies the pinned release and drops each raw wasm that ships a `.gz`
/// (the app + the geom kernel when present), so only the brotli/gzip variants + the
/// JS glue ride in the binary.
#[derive(RustEmbed)]
#[folder = "$OUT_DIR/fab-gui"]
struct FabGui;

/// The pinned bundle version (from build.rs), used to version-path the editor's
/// resource URLs so a bump busts the cache — same intent as the site's `?cb=`, but
/// in the PATH because the glue drops the query when resolving the wasm relatively.
const FAB_GUI_VERSION: &str = env!("FAB_GUI_VERSION");

/// The editor page, rendered UNDER the real site nav (CW.10). `templates/3d/editor.html`
/// owns the markup — the normal site header (jumbotron + nav partials, shared with
/// base.html) in document flow up top, then a `position: sticky` full-viewport tool
/// region: scroll down and the header slides up out of view while the fab-gui canvas
/// PINS at top:0 and fills the screen (the app draws its own MODEL/PARTS/EXPORT bar at
/// the canvas top, so it reads as the tool's sticky toolbar). The `<canvas id="fab-gui">`
/// MUST exist before `init()` (the app binds to it; `fit_canvas_to_parent` tracks
/// `#stage`), and the boot splash lifts on the app's `fab-gui:ready` event.
#[derive(Template)]
#[template(path = "3d/editor.html")]
struct EditorTemplate {
    top_bar: TopBar,
    auth_state: AuthenticationState,
    /// The version-pathed mount dir (`/3d/editor/<ver>/`) — `data-base`, so the app
    /// resolves lazy `geom/` + `libs.json` fetches against it, not the document URL.
    base: String,
    /// `<base>fab_gui.js` — the ES-module glue the document imports.
    glue_url: String,
}

/// COOP+COEP-carrying editor document. Isolation is scoped to `/3d/editor*`; the
/// rest of the site (content pages, media embeds, the model gallery) is never
/// cross-origin isolated. Every asset the page pulls (main.css, the nav's inline
/// icons, self-hosted fonts) is SAME-ORIGIN, so it survives COEP:require-corp.
async fn editor_document(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    // The bundle's mount base — reused for the glue import AND `data-base` (the app
    // resolves lazy geom/ + libs.json fetches against it, not the document URL).
    // Version in the path so it cache-busts + stays consistent.
    let viewer = session_data.auth_state.role();
    let base = format!("/3d/editor/{FAB_GUI_VERSION}/");
    let glue_url = format!("{base}fab_gui.js");
    let template = EditorTemplate {
        top_bar: TopBar::create(&state.pool, "3d", viewer).await?,
        auth_state: session_data.auth_state,
        base,
        glue_url,
    };
    let html = template
        .render()
        .map_err(|e| anyhow!("rendering the editor template: {e}"))?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header("cross-origin-opener-policy", "same-origin")
        .header("cross-origin-embedder-policy", "require-corp")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(html))
        .expect("editor document is a valid response"))
}

/// Serve ANY file from the version-pathed bundle tree (`fab_gui.js`, the wasm, the
/// `geom/` kernel worker + wasm, and the root `libs.json`). Prefers a precompressed
/// sibling the client accepts (`.br`, then `.gz`) with the matching
/// `Content-Encoding`; otherwise serves identity — reconstructing it by gunzipping
/// the `.gz` when the raw was dropped at build (build.rs drops a raw wasm only when
/// its `.gz` exists, so this fallback always has one). So a no-`Accept-Encoding`
/// client (curl, a proxy) always gets correct identity bytes, never a mislabeled
/// compressed blob. `{_v}` is ignored (the embed is always the current version; the
/// document only ever links the current path).
async fn editor_asset(
    Path((_v, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let ctype = bundle_content_type(&path);
    // A precompressed variant the client accepts, served with its Content-Encoding
    // (so it skips the site's on-the-fly CompressionLayer — never double-compressed).
    if accepts_encoding(&headers, "br") {
        if let Some(f) = FabGui::get(&format!("{path}.br")) {
            return bundle_response(f.data, &ctype, Some("br"));
        }
    }
    if accepts_encoding(&headers, "gzip") {
        if let Some(f) = FabGui::get(&format!("{path}.gz")) {
            return bundle_response(f.data, &ctype, Some("gzip"));
        }
    }
    // Identity: the file itself, or gunzip its `.gz` if the raw was dropped.
    if let Some(f) = FabGui::get(&path) {
        return bundle_response(f.data, &ctype, None);
    }
    if let Some(gz) = FabGui::get(&format!("{path}.gz")) {
        if let Ok(raw) = gunzip(&gz.data) {
            return bundle_response(raw.into(), &ctype, None);
        }
    }
    bundle_missing(&path)
}

/// `application/wasm` for `.wasm` (instantiateStreaming demands the exact MIME);
/// otherwise guess from the extension.
fn bundle_content_type(path: &str) -> String {
    if path.ends_with(".wasm") {
        "application/wasm".to_string()
    } else {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string()
    }
}

/// A version-pathed bundle-file response — immutable (the version is in the path).
fn bundle_response(
    data: std::borrow::Cow<'static, [u8]>,
    content_type: &str,
    encoding: Option<&str>,
) -> Response {
    let mut rb = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        // The editor document is COEP:require-corp, so its dedicated WORKER
        // (geom/geom-worker.js) must ITSELF be served require-corp or the load is
        // blocked — a same-origin module/wasm passes without these, but a Worker
        // script does not. CORP satisfies the resource-policy check; both are inert
        // on the non-worker files. (This is the CORP I wrongly dropped, thinking
        // same-origin was always exempt — true for subresources, NOT for workers.)
        .header("cross-origin-embedder-policy", "require-corp")
        .header("cross-origin-resource-policy", "same-origin");
    if let Some(enc) = encoding {
        rb = rb.header(header::CONTENT_ENCODING, enc);
    }
    rb.body(Body::from(data))
        .expect("embedded bundle file is a valid response")
}

/// Gunzip embedded bytes to reconstruct an identity file (the raw wasm dropped at
/// build) for a client that accepts no compression.
fn gunzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
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
    tracing::error!("fab-gui bundle file missing from the embed: {name}");
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from(format!("fab-gui bundle incomplete: {name}")))
        .expect("error response is valid")
}
