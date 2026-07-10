//! DI.2 — the page-write service. The create/update POLICY (slug, link-rewrite,
//! datetime parse, cover 3-way resolve, min_role decode, the update-then-set_cover
//! two-write, inherit-on-create) used to live inline in the axum handlers; it now
//! lives HERE so the editor handlers AND the MCP tools (Phase DI) share ONE code
//! path and can't drift. Handlers keep only HTTP concerns (form extraction, the
//! response); the typed outcome is `WrittenPage`.

use serde::Serialize;
use sqlx::SqlitePool;
use sqlx::types::chrono::{DateTime, NaiveDateTime, Utc};

use crate::db::dao::content_pages::ContentPageDao;
use crate::db::dao::roles::MinRole;
use crate::web::features::media::resolve_cover_media_id;
use crate::web::markdown::links::rewrite_site_links;
use crate::web::util::slug::Slug;

/// The typed result of a page write — the entity the handlers used to DISCARD
/// (they redirected/refreshed and dropped the DAO, so even the new slug had to be
/// recomputed downstream). DI.3's responder renders this per client.
#[derive(Debug, Clone, Serialize)]
pub struct WrittenPage {
    pub page_id: i64,
    /// URL slug (`page_name`).
    pub slug: String,
    /// Full pages-tree path from the root, e.g. `["blog", "my-post"]`.
    pub path_segments: Vec<String>,
    pub title: String,
    pub min_role: Option<String>,
    pub scheduled: bool,
    pub featured: bool,
}

impl WrittenPage {
    fn from_dao(cp: &ContentPageDao, path_segments: Vec<String>) -> Self {
        Self {
            page_id: cp.page_id,
            slug: cp.page_name.clone(),
            path_segments,
            title: cp.display_title(),
            min_role: cp.min_role.clone(),
            scheduled: cp.is_scheduled(),
            featured: cp.is_featured(),
        }
    }

    /// The canonical `/pages/<path>` URL for this page.
    pub fn pages_url(&self) -> String {
        format!("/pages/{}", self.path_segments.join("/"))
    }
}

/// Why a write couldn't complete. The caller maps each to ITS OWN response (an
/// axum handler → status + message; an MCP tool → a JSON-RPC error), so the exact
/// per-context wording stays with the caller.
#[derive(Debug)]
pub enum PageWriteError {
    /// The title slugified to empty (no letters or numbers). Create-only.
    EmptyTitle,
    /// The page (update) or the parent (create) path didn't resolve.
    NotFound,
    /// A create hit the `UNIQUE(parent_page_id, page_name)` constraint — a page
    /// with this slug already exists under `parent` (DK.1). Actionable, and it
    /// never leaks the raw SQLite constraint text to an MCP/HTTP caller.
    DuplicateSlug { slug: String, parent: String },
    /// A database / transform failure.
    Internal(anyhow::Error),
}

/// True when `e` (an `anyhow`-wrapped DAO error) is a SQLite UNIQUE-constraint
/// violation. The create path uses it to turn the raw `content_pages` constraint
/// into an actionable `DuplicateSlug` instead of a 500 with leaked schema text.
fn is_unique_violation(e: &anyhow::Error) -> bool {
    e.downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .is_some_and(|db| db.is_unique_violation())
}

/// Parse a `datetime-local` value (`YYYY-MM-DDTHH:MM[:SS]`) as a UTC instant. The
/// input carries no timezone, so it's stored as UTC verbatim (exact tz is
/// irrelevant for backdating an old post). `None` on empty/bad → the caller keeps
/// the existing date.
pub fn parse_local_datetime(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .ok()
        .map(|naive| naive.and_utc())
}

/// Create a page under `parent_path` (EMPTY = top-level) from a title. The slug is
/// derived; the page is born empty (content lands via a subsequent `update_page`).
/// `min_role` is INHERITED from the parent (top-level is born public — no parent).
pub async fn create_page(
    pool: &SqlitePool,
    parent_path: &[&str],
    title: &str,
) -> Result<WrittenPage, PageWriteError> {
    let title = title.trim().to_string();
    let slug = Slug::new(&title).ok_or(PageWriteError::EmptyTitle)?;

    let (parent_id, inherited_min_role, mut segments) = if parent_path.is_empty() {
        (None, None, Vec::new())
    } else {
        let parents = ContentPageDao::find_by_path(pool, parent_path)
            .await
            .map_err(PageWriteError::Internal)?;
        let parent = parents.last().ok_or(PageWriteError::NotFound)?;
        (
            Some(parent.page_id),
            parent.min_role.clone(),
            parent_path.iter().map(|s| (*s).to_string()).collect(),
        )
    };

    let mut cp =
        ContentPageDao::create(pool, parent_id, slug.as_str().to_string(), None, String::new(), None)
            .await
            .map_err(|e| {
                if is_unique_violation(&e) {
                    PageWriteError::DuplicateSlug {
                        slug: slug.as_str().to_string(),
                        parent: if parent_path.is_empty() {
                            "the top level".to_string()
                        } else {
                            parent_path.join("/")
                        },
                    }
                } else {
                    PageWriteError::Internal(e)
                }
            })?;
    cp.page_title = Some(title);
    cp.min_role = inherited_min_role;
    cp.update(pool).await.map_err(PageWriteError::Internal)?;

    segments.push(slug.into_string());
    Ok(WrittenPage::from_dao(&cp, segments))
}

/// A full-replace update of an existing page (mirrors the editor PUT — the editor
/// sends the WHOLE form). `title`/`category`/`markdown`/`order` are replaced
/// outright; `creation_date`/`min_role`/`cover_ref` carry three-valued
/// keep-semantics baked in (see the field docs). `Default` lets a partial caller
/// (an MCP tool doing read-modify-write) build it ergonomically.
#[derive(Debug, Default)]
pub struct PageUpdate {
    /// Replaces `page_title` (`None` clears it).
    pub title: Option<String>,
    /// Replaces `page_category` (`None` clears it).
    pub category: Option<String>,
    /// Replaces `page_markdown` (link-rewritten to root-relative on save).
    pub markdown: String,
    /// Replaces `page_order`.
    pub order: i64,
    /// `datetime-local` override; empty/unparseable / `None` → KEEP the existing date.
    pub creation_date: Option<String>,
    /// `Some("Public")` → clear the gate; `Some(known role)` → set it; `None` or
    /// anything unrecognized → KEEP (never silently loosen visibility).
    pub min_role: Option<String>,
    /// `None` → CLEAR the cover; a resolvable ref → set it; a non-empty but
    /// unresolvable ref → KEEP (a typo must not wipe an existing cover). NOTE the
    /// asymmetry with `min_role`: an absent cover CLEARS, an absent min_role KEEPS
    /// — this preserves the editor's exact behavior.
    pub cover_ref: Option<String>,
}

pub async fn update_page(
    pool: &SqlitePool,
    site_host: &str,
    path: &[&str],
    input: PageUpdate,
) -> Result<WrittenPage, PageWriteError> {
    // Resolve the cover FIRST (matches the handler's order), three-valued:
    //   None         → Some(None)      = clear
    //   resolvable   → Some(Some(id))  = set
    //   unresolvable → None            = skip (preserve existing)
    let cover_update: Option<Option<i64>> = match &input.cover_ref {
        None => Some(None),
        Some(raw) => resolve_cover_media_id(pool, raw).await.map(Some),
    };

    let pages_path = ContentPageDao::find_by_path(pool, path)
        .await
        .map_err(PageWriteError::Internal)?;
    let mut lp = pages_path.last().ok_or(PageWriteError::NotFound)?.to_owned();

    lp.page_title = input.title;
    lp.page_category = input.category;
    lp.page_markdown =
        rewrite_site_links(&input.markdown, site_host).map_err(PageWriteError::Internal)?;
    lp.page_order = input.order;
    if let Some(dt) = input.creation_date.as_deref().and_then(parse_local_datetime) {
        lp.page_creation_date = dt;
    }
    lp.min_role = MinRole::from_stored(lp.min_role.as_deref())
        .apply_write(input.min_role.as_deref())
        .to_stored();
    lp.update(pool).await.map_err(PageWriteError::Internal)?;

    // Cover is a separate column `update()` doesn't touch; skip entirely on an
    // unresolvable ref so the existing cover is preserved.
    if let Some(cover_media_id) = cover_update {
        ContentPageDao::set_cover(pool, lp.page_id, cover_media_id)
            .await
            .map_err(PageWriteError::Internal)?;
    }

    Ok(WrittenPage::from_dao(
        &lp,
        path.iter().map(|s| (*s).to_string()).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fetch(pool: &SqlitePool, path: &[&str]) -> ContentPageDao {
        ContentPageDao::find_by_path(pool, path)
            .await
            .unwrap()
            .pop()
            .expect("page exists")
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn create_top_level_is_public_and_slugged(pool: SqlitePool) {
        let w = create_page(&pool, &[], "My New Page!").await.unwrap();
        assert_eq!(w.slug, "my-new-page");
        assert_eq!(w.title, "My New Page!");
        assert_eq!(w.min_role, None);
        assert!(!w.scheduled);
        assert_eq!(w.pages_url(), "/pages/my-new-page");
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn create_empty_title_rejected(pool: SqlitePool) {
        let err = create_page(&pool, &[], "   !!!  ").await.unwrap_err();
        assert!(matches!(err, PageWriteError::EmptyTitle));
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn create_under_missing_parent_is_not_found(pool: SqlitePool) {
        let err = create_page(&pool, &["nope"], "Child").await.unwrap_err();
        assert!(matches!(err, PageWriteError::NotFound));
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn child_inherits_parent_gate(pool: SqlitePool) {
        create_page(&pool, &[], "Vault").await.unwrap();
        update_page(
            &pool,
            "hotchkiss.io",
            &["vault"],
            PageUpdate {
                title: Some("Vault".into()),
                min_role: Some("Family".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let child = create_page(&pool, &["vault"], "Secret").await.unwrap();
        assert_eq!(child.min_role.as_deref(), Some("Family"));
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn update_min_role_is_three_valued(pool: SqlitePool) {
        create_page(&pool, &[], "P").await.unwrap();
        let up = |min_role: Option<String>| PageUpdate {
            title: Some("P".into()),
            markdown: "body".into(),
            min_role,
            ..Default::default()
        };
        // set
        update_page(&pool, "h", &["p"], up(Some("Registered".into())))
            .await
            .unwrap();
        assert_eq!(fetch(&pool, &["p"]).await.min_role.as_deref(), Some("Registered"));
        // keep (absent)
        update_page(&pool, "h", &["p"], up(None)).await.unwrap();
        assert_eq!(fetch(&pool, &["p"]).await.min_role.as_deref(), Some("Registered"));
        // keep (unrecognized — must never silently loosen)
        update_page(&pool, "h", &["p"], up(Some("Bogus".into())))
            .await
            .unwrap();
        assert_eq!(fetch(&pool, &["p"]).await.min_role.as_deref(), Some("Registered"));
        // clear (Public)
        update_page(&pool, "h", &["p"], up(Some("Public".into())))
            .await
            .unwrap();
        assert_eq!(fetch(&pool, &["p"]).await.min_role, None);
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn update_backdates_creation_date(pool: SqlitePool) {
        create_page(&pool, &[], "P").await.unwrap();
        update_page(
            &pool,
            "h",
            &["p"],
            PageUpdate {
                title: Some("P".into()),
                markdown: "b".into(),
                creation_date: Some("2012-03-04T05:06".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let p = fetch(&pool, &["p"]).await;
        assert_eq!(
            p.page_creation_date.format("%Y-%m-%d %H:%M").to_string(),
            "2012-03-04 05:06"
        );
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn update_missing_page_is_not_found(pool: SqlitePool) {
        let err = update_page(&pool, "h", &["ghost"], PageUpdate::default())
            .await
            .unwrap_err();
        assert!(matches!(err, PageWriteError::NotFound));
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn update_unresolvable_cover_is_a_noop_not_an_error(pool: SqlitePool) {
        create_page(&pool, &[], "P").await.unwrap();
        // A garbage cover ref must SKIP (preserve), never error or wipe.
        update_page(
            &pool,
            "h",
            &["p"],
            PageUpdate {
                title: Some("P".into()),
                markdown: "b".into(),
                cover_ref: Some("not-a-real-ref".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
}
