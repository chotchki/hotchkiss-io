use axum::{
    middleware::from_fn,
    routing::{delete, get, post},
    Router,
};

use crate::web::{app_state::AppState, middleware::require_admin::require_admin};

pub mod analytics;
pub mod api_keys;
pub mod capture;
pub mod dead_links;
pub mod greylist;
pub mod logs;
pub mod manga_ingest;
pub mod media;
pub mod pages;
pub mod users;

/// Everything under `/admin` — gated as a group by the `require_admin` layer,
/// so handlers inside don't repeat the check.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/analytics", get(analytics::show_analytics))
        // Per-IP drill-down (CQ.4): scan-fingerprint detail for one IP.
        .route("/analytics/ip/{ip}", get(analytics::show_ip_detail))
        // Recompute stored is_bot over all rows (CR.2.1) — retune-on-demand.
        .route(
            "/analytics/reclassify-bots",
            post(analytics::reclassify_bots),
        )
        .route(
            "/capture",
            get(capture::show_capture).post(capture::capture_post),
        )
        .route("/pages", get(pages::show_admin_pages))
        .route("/pages/reorder", post(pages::reorder_pages))
        // Child-index widget drag-reorder (DV.12) — a fixed two-segment path, so it
        // never collides with `/pages/{page_id}/...`.
        .route("/pages/reorder-children", post(pages::reorder_children))
        // Toggle a page's landing "Featured" pin (13.8). Two path segments, so it
        // never collides with the static `/pages/reorder`.
        .route("/pages/{page_id}/feature", post(pages::toggle_feature))
        // Publish a scheduled/draft page now, or unpublish a live page back to a
        // draft (Phase CU) — same two-path-segment shape as /feature.
        .route("/pages/{page_id}/publish", post(pages::publish_now))
        .route("/pages/{page_id}/unpublish", post(pages::unpublish))
        // Bulk book (EPUB/CBZ) import (Phase DW): the console + the two front doors —
        // `/filesystem` (server folder, spawned) + `/upload` (browser drop, synchronous).
        // Lives under `/admin/media/` (it creates media items); linked from the media
        // library. `manga_ingest` is the module (it ingests books into a series).
        .route("/media/import", get(manga_ingest::show_ingest_console))
        .route("/media/import/filesystem", post(manga_ingest::ingest_filesystem))
        .route("/media/import/upload", post(manga_ingest::ingest_upload))
        // Backfill covers for books imported before the EPUB first-image fallback (DW.11).
        .route(
            "/media/import/backfill-covers",
            post(manga_ingest::backfill_covers),
        )
        // Media library PAGE (Phase BZ) — the HTML admin UI. All MUTATIONS moved to
        // the canonical `/media` REST surface (Phase DR): the library JS drives
        // `POST /media`, `POST /media/<ref>/variants`, `PUT`/`DELETE /media/<ref>`,
        // `DELETE /media/<ref>/variants/<url_key>`. This route is the page only,
        // distinct from `GET /media` (the JSON collection).
        .route("/media", get(media::show_media_library))
        .route("/media/{ref}", get(media::show_media_edit))
        .route("/media/{ref}/rederive", post(media::rederive_media))
        .route("/media/{ref}/rotate", post(media::rotate_media))
        .route("/media/{ref}/crop", post(media::crop_media))
        // API keys (Phase CA): generate (shown once) / list / revoke your own.
        .route(
            "/api-keys",
            get(api_keys::show_api_keys).post(api_keys::create_api_key),
        )
        .route("/api-keys/{id}", delete(api_keys::revoke_api_key))
        // User management (Phase CC): list / promote-demote / delete.
        .route("/users", get(users::show_users))
        .route("/users/{id}/role", post(users::set_user_role))
        .route("/users/{id}", delete(users::delete_user))
        // Server log tail (Phase CO): manual-refresh viewer; excluded from
        // request_log (request_log.rs) so a self-view never feeds the access log.
        .route("/logs", get(logs::show_logs))
        // Greylist management (Phase CX): view/pin/release. `/pin` is a fixed segment
        // (one path element), `{ip}/release` is two — no collision.
        .route("/greylist", get(greylist::show_greylist))
        .route("/greylist/pin", post(greylist::pin_ip))
        .route("/greylist/run-sweep", post(greylist::run_sweep))
        .route("/greylist/{ip}/release", post(greylist::release_ip))
        // Dead-link checker (Phase DL): the report, a full re-scan, a per-link re-check.
        .route("/dead-links", get(dead_links::show_dead_links))
        .route("/dead-links/run-scan", post(dead_links::run_scan))
        .route("/dead-links/recheck", post(dead_links::recheck))
        .route("/dead-links/ignore", post(dead_links::ignore))
        .route("/dead-links/unignore", post(dead_links::unignore))
        .layer(from_fn(require_admin))
}
