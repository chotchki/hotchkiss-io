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

/// The stable placeholder the transformer emits for a ` ```children ` fence, encoding
/// the ordering plus the card aspect (DZ.1) so the fill picks the matching render. Both
/// are small closed-set tokens (`parse_order`/`parse_aspect`), so the fill does an
/// exact-string match then replace, with no HTML parsing. The aspect rides the sentinel
/// so it's part of the content-hash cache key — an `aspect=` edit re-transforms.
pub fn sentinel(order: &str, aspect: &str) -> String {
    format!("<div class=\"child-index\" data-order=\"{order}\" data-aspect=\"{aspect}\"></div>")
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

/// Parse the fence META for the card aspect (DZ.1: ` ```children aspect=square `) →
/// `"square"` | `"portrait"`. Default `"portrait"` (the legacy 3:4 book cover) so no
/// existing section changes; `square` is the audiobook/album-art opt-in (a square
/// cover in a 3:4 box gets cropped top+bottom by object-cover).
pub fn parse_aspect(meta: Option<&str>) -> &'static str {
    for tok in meta.unwrap_or("").split_whitespace() {
        if let Some(v) = tok.strip_prefix("aspect=") {
            if v.eq_ignore_ascii_case("square") {
                return "square";
            }
            if v.eq_ignore_ascii_case("portrait") {
                return "portrait";
            }
        }
    }
    "portrait"
}

/// Map an order token to the DAO ordering — the ONE place the token↔enum mapping
/// lives (shared by `fill` and the library code routes that read a section's fence).
pub fn list_order(token: &str) -> ListOrder {
    if token == "newest" {
        ListOrder::Newest
    } else {
        ListOrder::Ordered
    }
}

/// Extract the ` ```children ` fence's META line from a page's markdown, if present —
/// so a CODE route (the `/library` index + `/library/<section>` list-base) can honor
/// the SAME `order`/`aspect` the author wrote in the section page, instead of
/// hardcoding. Returns the text after `children` on the fence line (e.g.
/// `"order=newest aspect=square"`); `None` when the page carries no children fence.
pub fn children_fence_meta(markdown: &str) -> Option<String> {
    for line in markdown.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("```")
            && let Some(meta) = rest.trim_start().strip_prefix("children")
        {
            return Some(meta.trim().to_string());
        }
    }
    None
}

struct ChildCard {
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
    /// Card cover-box shape (DZ.1): `true` → `aspect-square` (audiobook/album art),
    /// `false` → the `aspect-[3/4]` portrait book default. A bool (not the class
    /// string) because Tailwind only extracts the arbitrary-value class from a literal
    /// in the template, so the template branches on this and writes both class names.
    is_square: bool,
}

/// Render the paginated children grid for `parent` as an HTML fragment — the ONE
/// card/grid renderer, shared by the ` ```children ` fence fill (content pages) AND
/// the generic library section route. `list_base` builds the pager links (the URL
/// the viewer is ON); `child_base` builds the card links + the new-child form action
/// (the content-tree path children live under). On a content page the two are equal.
// Eight bind-once render inputs (pool, parent, query, order, two bases, viewer,
// aspect) — all distinct, none groupable into a meaningful struct without obscuring
// the call sites, so the arg count is deliberate.
#[allow(clippy::too_many_arguments)]
pub async fn render_children_grid(
    pool: &SqlitePool,
    parent_page_id: i64,
    query: &ListingQuery,
    order: ListOrder,
    list_base: &str,
    child_base: &str,
    viewer: Role,
    aspect: &str,
) -> Result<String> {
    let (items, pagination) =
        paginate(pool, Some(parent_page_id), query, order, list_base, viewer).await?;
    let mut cards = Vec::with_capacity(items.len());
    for c in items {
        // Explicit page cover wins; else derive it from the page's first media embed
        // (DV.11) so a book/volume auto-covers with no manual cover-setting; else, for a
        // CONTAINER card (a manga series — only a ` ```children ` fence, no embed), roll
        // up to its first volume's cover (DW.12) so the series tile isn't blank.
        let cover_url = match crate::web::features::media::cover_url_for(pool, c.page_id).await {
            Some(url) => Some(url),
            None => match crate::web::features::media::embedded_media_cover(pool, &c.page_markdown)
                .await
            {
                Some(url) => Some(url),
                None => {
                    crate::web::features::media::child_rollup_cover(pool, c.page_id, viewer).await
                }
            },
        };
        cards.push(ChildCard {
            title: c.display_title(),
            url: format!("{child_base}/{}", c.page_name),
            cover_url,
            excerpt: cached_excerpt(&c.page_markdown),
            scheduled: c.is_scheduled(),
            visibility: c.visibility_label(),
        });
    }
    Ok(ChildIndexTemplate {
        cards,
        pagination,
        is_admin: viewer == Role::Admin,
        form_action: child_base.to_string(),
        is_square: aspect == "square",
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
    // The sentinel encodes order × aspect (DZ.1) — a small closed set, so match each
    // combo's exact marker string + replace (no HTML parsing). A `data-aspect` an older
    // cached sentinel lacks simply never matches "portrait"/"square" here, so it falls
    // through untouched — but `sentinel()` always writes both, so fresh transforms hit.
    for (token, order) in [("newest", ListOrder::Newest), ("manual", ListOrder::Ordered)] {
        for aspect in ["portrait", "square"] {
            let marker = sentinel(token, aspect);
            if !html.contains(&marker) {
                continue;
            }
            let grid = render_children_grid(
                pool,
                parent_page_id,
                query,
                order,
                base_path,
                base_path,
                viewer,
                aspect,
            )
            .await?;
            html = html.replace(&marker, &grid);
        }
    }
    Ok(html)
}
