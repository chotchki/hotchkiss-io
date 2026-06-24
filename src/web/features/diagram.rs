//! The HTMX swap target for inline diagrams.
//!
//! A page renders a ` ```d2 ` fence as a placeholder that carries the d2 source
//! and `hx-get="/diagram/<hash>"` (see `web/markdown/diagram.rs`). On load HTMX
//! GETs here; we render the SVG for that hash and return it for the swap.
//!
//! Renders ONLY sources the server already emitted (looked up by content hash),
//! so this is not an open "compile arbitrary d2" endpoint. Always 200 — a bad
//! source or a stale hash returns a visible error block so HTMX still swaps in
//! something rather than leaving the raw source on screen.

use axum::extract::Path;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::response::Response;

use crate::web::markdown::diagram;

pub async fn render_registered_diagram(Path(hash): Path<String>) -> Response {
    let html = diagram::render_registered(&hash).unwrap_or_else(|| {
        diagram::error_block("d2", "diagram not found — the page may need a reload")
    });
    Html(html).into_response()
}
