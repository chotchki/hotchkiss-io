//! The child-index markdown widget (Phase DV) — THE listing mechanism.
//!
//! A ` ```children ` fence in a content page's markdown becomes a paginated,
//! searchable grid of that page's CHILD pages. The transformer emits a STABLE
//! sentinel (encoding the ordering) so `transform()` stays pure + content-hash
//! cached; THIS module fills it PER-REQUEST in the page render, where the page
//! identity + pool + viewer are known.
//!
//! It's flexible by design: `order=newest` (blog/audiobooks) vs `order=manual`
//! (projects/manga volumes), one rich card (cover + title + excerpt + Scheduled /
//! visibility badges), an admin "+ new child" form, and the shared
//! `listing_search` / `listing_pager` partials. Audiobooks + manga are both just a
//! content page carrying this fence — no per-section handler or template.

use anyhow::Result;
use askama::Template;
use sqlx::SqlitePool;

use crate::db::dao::roles::Role;
use crate::web::features::listing::{paginate, ListOrder, ListingQuery, Pagination};
use crate::web::markdown::render_cache::cached_excerpt;

/// The stable placeholder the transformer emits for a ` ```children ` fence,
/// encoding the ordering so the fill picks the matching one. `order` is one of the
/// tokens `parse_order` yields (`"newest"` / `"manual"`) — a small closed set, so
/// the fill can exact-string-match + replace without parsing HTML.
pub fn sentinel(order: &str) -> String {
    format!("<div class=\"child-index\" data-order=\"{order}\"></div>")
}

/// Parse the fence META (` ```children order=newest `) → the ordering token.
/// Default `"manual"` (`page_order` — the curated / volume-number case).
pub fn parse_order(meta: Option<&str>) -> &'static str {
    for tok in meta.unwrap_or("").split_whitespace() {
        if let Some(v) = tok.strip_prefix("order=") {
            if v.eq_ignore_ascii_case("newest") {
                return "newest";
            }
            if v.eq_ignore_ascii_case("manual") || v.eq_ignore_ascii_case("ordered") {
                return "manual";
            }
        }
    }
    "manual"
}

struct ChildCard {
    /// The child page's id — the drag-reorder hidden input (DV.12).
    page_id: i64,
    title: String,
    /// `{base_path}/{child slug}` — the child page's URL.
    url: String,
    cover_url: Option<String>,
    excerpt: String,
    /// Admin-preview badges (an insufficient viewer never receives these children,
    /// so the badges only ever render for a viewer allowed to see them).
    scheduled: bool,
    visibility: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "partials/child_index.html")]
struct ChildIndexTemplate {
    cards: Vec<ChildCard>,
    pagination: Pagination,
    /// Show the "+ new child" authoring form + the drag-reorder handles (DV.12).
    is_admin: bool,
    /// The content-tree path children live under — the "+ new child" form `hx-post`s
    /// here to create a child. Distinct from the pager base (`pagination.base_path`):
    /// on a content page they're the same (`/pages/<path>`), but the library section
    /// route lists at `/library/<section>` while children live at `/pages/library/<section>`.
    form_action: String,
    /// The parent page id + the page-order offset for the drag-reorder POST (DV.12):
    /// within-page drag writes `reorder_start..+N`, so pagination stays consistent.
    parent_id: i64,
    reorder_start: i64,
}

/// Render the paginated children grid for `parent` as an HTML fragment — the ONE
/// card/grid renderer, shared by the ` ```children ` fence fill (content pages) AND
/// the generic library section route. `list_base` builds the pager links (the URL
/// the viewer is ON); `child_base` builds the card links + the new-child form action
/// (the content-tree path children live under). On a content page the two are equal.
pub async fn render_children_grid(
    pool: &SqlitePool,
    parent_page_id: i64,
    query: &ListingQuery,
    order: ListOrder,
    list_base: &str,
    child_base: &str,
    viewer: Role,
) -> Result<String> {
    let (items, pagination) =
        paginate(pool, Some(parent_page_id), query, order, list_base, viewer).await?;
    let mut cards = Vec::with_capacity(items.len());
    for c in items {
        // Explicit page cover wins; else derive it from the page's first media embed
        // (DV.11) so a book/volume auto-covers with no manual cover-setting.
        let cover_url = match crate::web::features::media::cover_url_for(pool, c.page_id).await {
            Some(url) => Some(url),
            None => crate::web::features::media::embedded_media_cover(pool, &c.page_markdown).await,
        };
        cards.push(ChildCard {
            page_id: c.page_id,
            title: c.display_title(),
            url: format!("{child_base}/{}", c.page_name),
            cover_url,
            excerpt: cached_excerpt(&c.page_markdown),
            scheduled: c.is_scheduled(),
            visibility: c.visibility_label(),
        });
    }
    let reorder_start = (pagination.current_page - 1) * crate::web::features::listing::PAGE_SIZE;
    Ok(ChildIndexTemplate {
        cards,
        pagination,
        is_admin: viewer == Role::Admin,
        form_action: child_base.to_string(),
        parent_id: parent_page_id,
        reorder_start,
    }
    .render()?)
}

/// If `html` carries a child-index sentinel, replace it with the rendered children
/// grid (ordering per the sentinel, gated to `viewer`, paginated + `?q=` searchable).
/// No sentinel → the HTML is returned untouched (the common case — two cheap string
/// searches). On a content page the pager + child bases are the same `base_path`.
pub async fn fill(
    mut html: String,
    parent_page_id: i64,
    pool: &SqlitePool,
    query: &ListingQuery,
    base_path: &str,
    viewer: Role,
) -> Result<String> {
    for (token, order) in [("newest", ListOrder::Newest), ("manual", ListOrder::Ordered)] {
        let marker = sentinel(token);
        if !html.contains(&marker) {
            continue;
        }
        let grid =
            render_children_grid(pool, parent_page_id, query, order, base_path, base_path, viewer)
                .await?;
        html = html.replace(&marker, &grid);
    }
    Ok(html)
}
