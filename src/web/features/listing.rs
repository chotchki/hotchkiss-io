//! Generic paginated + searchable listing of a content page's children, shared
//! by `/blog` (newest-first) and `/projects` (manual `page_order`). The DAO does
//! the count + paged fetch + `LIKE` search filter (see
//! `ContentPageDao::count_children` / `find_children_*_paged`); this builds the
//! `Pagination` that both index templates render via
//! `partials/listing_controls.html`. Each page keeps its own card type +
//! ordering; only the windowing/search machinery is shared.

use anyhow::Result;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::db::dao::content_pages::ContentPageDao;
use crate::db::dao::roles::Role;
use crate::web::util::urlencode::urlencode;

/// Items per page.
pub const PAGE_SIZE: i64 = 10;

/// Which DAO ordering a listing uses.
pub enum ListOrder {
    /// Newest by `page_creation_date` (blog).
    Newest,
    /// Manual `page_order` (projects).
    Ordered,
}

/// `?q=…&page=N` query params for a listing — both optional.
#[derive(Deserialize, Default)]
pub struct ListingQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub page: Option<i64>,
}

/// Everything `listing_controls.html` needs: the current window, the active
/// search, and the base path used to build prev/next links + the search form
/// action.
pub struct Pagination {
    /// 1-indexed, clamped into `[1, total_pages]`.
    pub current_page: i64,
    /// Always `>= 1`.
    pub total_pages: i64,
    pub total_results: i64,
    /// The active query (`""` = none).
    pub search: String,
    /// e.g. `/blog`.
    pub base_path: String,
}

impl Pagination {
    pub fn has_prev(&self) -> bool {
        self.current_page > 1
    }
    pub fn has_next(&self) -> bool {
        self.current_page < self.total_pages
    }
    pub fn is_search(&self) -> bool {
        !self.search.is_empty()
    }
    /// More than one page to navigate.
    pub fn is_paged(&self) -> bool {
        self.total_pages > 1
    }
    pub fn prev_url(&self) -> String {
        self.page_url(self.current_page - 1)
    }
    pub fn next_url(&self) -> String {
        self.page_url(self.current_page + 1)
    }
    /// Build `base_path?q=…&page=N`, omitting `q` when empty and `page` when 1.
    fn page_url(&self, page: i64) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.search.is_empty() {
            parts.push(format!("q={}", urlencode(&self.search)));
        }
        if page > 1 {
            parts.push(format!("page={page}"));
        }
        if parts.is_empty() {
            self.base_path.clone()
        } else {
            format!("{}?{}", self.base_path, parts.join("&"))
        }
    }
}

/// Fetch one page of `parent`'s children (ordered per `order`, filtered by the
/// trimmed query) plus the `Pagination` describing the window. `page` is clamped
/// into `[1, total_pages]`; an empty/whitespace query disables the search filter.
/// `viewer` gates BOTH unpublished (future-dated, Phase CU) and role-gated
/// (`min_role`, Phase DA) children — an insufficient viewer never sees OR counts
/// them, so the window and the count stay consistent.
pub async fn paginate(
    pool: &SqlitePool,
    parent_page_id: Option<i64>,
    query: &ListingQuery,
    order: ListOrder,
    base_path: &str,
    viewer: Role,
) -> Result<(Vec<ContentPageDao>, Pagination)> {
    let search = query.q.as_deref().unwrap_or("").trim().to_string();

    let total = ContentPageDao::count_children(pool, parent_page_id, &search, viewer).await?;
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let current_page = query.page.unwrap_or(1).clamp(1, total_pages);
    let offset = (current_page - 1) * PAGE_SIZE;

    let items = match order {
        ListOrder::Newest => {
            ContentPageDao::find_children_newest_paged(
                pool,
                parent_page_id,
                &search,
                PAGE_SIZE,
                offset,
                viewer,
            )
            .await?
        }
        ListOrder::Ordered => {
            ContentPageDao::find_children_ordered_paged(
                pool,
                parent_page_id,
                &search,
                PAGE_SIZE,
                offset,
                viewer,
            )
            .await?
        }
    };

    Ok((
        items,
        Pagination {
            current_page,
            total_pages,
            total_results: total,
            search,
            base_path: base_path.to_string(),
        },
    ))
}
