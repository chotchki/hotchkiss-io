use anyhow::Result;
use sqlx::{
    prelude::FromRow,
    query, query_as,
    types::chrono::{self, DateTime, Utc},
    SqliteExecutor,
};

use super::roles::{MinRole, Role};

#[derive(Clone, Debug, FromRow, PartialEq)]
pub struct ContentPageDao {
    pub page_id: i64,
    pub parent_page_id: Option<i64>,
    pub page_name: String,
    pub page_title: Option<String>,
    pub page_category: Option<String>,
    pub page_markdown: String,
    pub page_cover_attachment_id: Option<i64>,
    pub page_order: i64,
    pub page_creation_date: chrono::DateTime<Utc>,
    pub page_modified_date: chrono::DateTime<Utc>,
    pub special_page: bool,
    /// Minimum role that may READ this page (Phase DA). `None` is the ONLY
    /// public spelling; see `min_role_rank` for the fail-closed decode.
    pub min_role: Option<String>,
}

impl ContentPageDao {
    pub async fn create(
        executor: impl SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
        page_name: String,
        page_category: Option<String>,
        page_markdown: String,
        page_cover_attachment_id: Option<i64>,
    ) -> Result<ContentPageDao> {
        let result = query!(
            r#"
        INSERT INTO content_pages (
            parent_page_id,
            page_name,
            page_category,
            page_markdown,
            page_cover_attachment_id
        ) VALUES (
            ?1,
            ?2,
            ?3,
            ?4,
            ?5
        ) RETURNING 
            page_id,
            page_order,
            page_creation_date as "page_creation_date: DateTime<Utc>",
            page_modified_date as "page_modified_date: DateTime<Utc>",
            special_page
        "#,
            parent_page_id,
            page_name,
            page_category,
            page_markdown,
            page_cover_attachment_id
        )
        .fetch_one(executor)
        .await?;

        Ok(ContentPageDao {
            page_id: result.page_id,
            parent_page_id,
            page_name,
            page_title: None,
            page_category,
            page_markdown,
            page_cover_attachment_id,
            page_order: result.page_order,
            page_creation_date: result.page_creation_date,
            page_modified_date: result.page_modified_date,
            special_page: result.special_page,
            min_role: None,
        })
    }

    pub async fn delete(&self, executor: impl SqliteExecutor<'_>) -> Result<()> {
        query!(
            r#"
            DELETE FROM content_pages
            WHERE page_id = ?1
            and special_page = false
            "#,
            self.page_id
        )
        .execute(executor)
        .await?;

        Ok(())
    }

    pub async fn update(&mut self, executor: impl SqliteExecutor<'_>) -> Result<()> {
        let result = query!(
            r#"
        UPDATE content_pages
        SET
            parent_page_id = ?1,
            page_name = ?2,
            page_title = ?3,
            page_category = ?4,
            page_markdown = ?5,
            page_cover_attachment_id = ?6,
            page_order = ?7,
            page_creation_date = ?8,
            min_role = ?9,
            page_modified_date = datetime('now', 'utc')
        WHERE
            page_id = ?10
        RETURNING
            page_modified_date as "page_modified_date: DateTime<Utc>"
        "#,
            self.parent_page_id,
            self.page_name,
            self.page_title,
            self.page_category,
            self.page_markdown,
            self.page_cover_attachment_id,
            self.page_order,
            self.page_creation_date,
            self.min_role,
            self.page_id
        )
        .fetch_one(executor)
        .await?;

        self.page_modified_date = result.page_modified_date;

        Ok(())
    }

    /// Set just the cover media for one page, STAMPING `page_modified_date`. A
    /// cover IS a content change — it drives the card thumbnail + `og:image`, and
    /// the feed / sitemap key their validators (and the feed's `<updated>`) off
    /// `page_modified_date`, so a cover-only change must move it or those go
    /// stale. (Contrast `set_order`, which deliberately does NOT stamp — reordering
    /// isn't a content change and doesn't affect the feed, ordered by date.) A
    /// NULL `media_id` clears the cover.
    pub async fn set_cover(
        executor: impl SqliteExecutor<'_>,
        page_id: i64,
        media_id: Option<i64>,
    ) -> Result<()> {
        query!(
            "UPDATE content_pages SET page_cover_media_id = ?1, page_modified_date = datetime('now', 'utc') WHERE page_id = ?2",
            media_id,
            page_id
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Set just the ordering for one page — used by drag-to-reorder, which sets
    /// `page_order` to the page's position in the dragged list. `page_order`
    /// drives both the nav tab order and the Manage Pages list.
    pub async fn set_order(
        executor: impl SqliteExecutor<'_>,
        page_id: i64,
        page_order: i64,
    ) -> Result<()> {
        query!(
            "UPDATE content_pages SET page_order = ?1 WHERE page_id = ?2",
            page_order,
            page_id
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Set just `page_category` for one page — the pin button's targeted write
    /// (Phase 13.8). Deliberately does NOT stamp `page_modified_date` (like
    /// `set_order`): featuring is a landing-curation flag, not a content edit, so
    /// it must not churn the feed/sitemap validators or the `<updated>` date.
    pub async fn set_category(
        executor: impl SqliteExecutor<'_>,
        page_id: i64,
        page_category: Option<String>,
    ) -> Result<()> {
        query!(
            "UPDATE content_pages SET page_category = ?1 WHERE page_id = ?2",
            page_category,
            page_id
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Set just `page_creation_date` (the publish instant) for one page — the
    /// Publish-now / Unpublish buttons' targeted write (Phase CU). Unlike
    /// `set_category`/`set_order` this DOES stamp `page_modified_date`: publishing
    /// or unpublishing changes the page's feed/sitemap position + visibility, so the
    /// validators (and the feed's `<updated>`) must move. Writes ONE column against
    /// the current DB row (like the Pin button), so it never clobbers unsaved editor
    /// markdown the way the whole-row `update()` would.
    pub async fn set_creation_date(
        executor: impl SqliteExecutor<'_>,
        page_id: i64,
        page_creation_date: DateTime<Utc>,
    ) -> Result<()> {
        query!(
            "UPDATE content_pages SET page_creation_date = ?1, page_modified_date = datetime('now', 'utc') WHERE page_id = ?2",
            page_creation_date,
            page_id
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    pub async fn find_by_parent(
        executor: impl SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
    ) -> Result<Vec<ContentPageDao>> {
        let content_pages: Vec<ContentPageDao> = query_as!(
            ContentPageDao,
            r#"
                select 
                    page_id as "page_id!",
                    parent_page_id,
                    page_name,
                    page_title,
                    page_category,
                    page_markdown,
                    page_cover_attachment_id as "page_cover_attachment_id?",
                    page_order,
                    page_creation_date as "page_creation_date: DateTime<Utc>",
                    page_modified_date as "page_modified_date: DateTime<Utc>",
                    special_page,
                    min_role
                from
                    content_pages
                where
                    parent_page_id IS ?1
                order by page_order
        "#,
            parent_page_id
        )
        .fetch_all(executor)
        .await?;

        Ok(content_pages)
    }

    pub async fn find_by_id(
        executor: impl sqlx::SqliteExecutor<'_>,
        page_id: i64,
    ) -> Result<Option<ContentPageDao>> {
        let content_page: Option<ContentPageDao> = query_as!(
            ContentPageDao,
            r#"
                select 
                    page_id,
                    parent_page_id,
                    page_name,
                    page_title,
                    page_category,
                    page_markdown,
                    page_cover_attachment_id,
                    page_order,
                    page_creation_date as "page_creation_date: DateTime<Utc>",
                    page_modified_date as "page_modified_date: DateTime<Utc>",
                    special_page,
                    min_role
                from
                    content_pages
                where
                    page_id = ?1
                order by page_order
        "#,
            page_id
        )
        .fetch_optional(executor)
        .await?;

        Ok(content_page)
    }

    pub async fn find_by_name(
        executor: impl sqlx::SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
        page_name: &str,
    ) -> Result<Option<ContentPageDao>> {
        let content_page: Option<ContentPageDao> = query_as!(
            ContentPageDao,
            r#"
                select 
                    page_id as "page_id!",
                    parent_page_id,
                    page_name,
                    page_title,
                    page_category,
                    page_markdown,
                    page_cover_attachment_id,
                    page_order,
                    page_creation_date as "page_creation_date: DateTime<Utc>",
                    page_modified_date as "page_modified_date: DateTime<Utc>",
                    special_page,
                    min_role
                from
                    content_pages
                where
                    parent_page_id IS ?1
                    and page_name = ?2
                order by page_order
        "#,
            parent_page_id,
            page_name
        )
        .fetch_optional(executor)
        .await?;

        Ok(content_page)
    }

    pub async fn find_by_parent_newest_first(
        executor: impl SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
        limit: Option<i64>,
    ) -> Result<Vec<ContentPageDao>> {
        let limit = limit.unwrap_or(i64::MAX);
        let content_pages: Vec<ContentPageDao> = query_as!(
            ContentPageDao,
            r#"
                select
                    page_id as "page_id!",
                    parent_page_id,
                    page_name,
                    page_title,
                    page_category,
                    page_markdown,
                    page_cover_attachment_id as "page_cover_attachment_id?",
                    page_order,
                    page_creation_date as "page_creation_date: DateTime<Utc>",
                    page_modified_date as "page_modified_date: DateTime<Utc>",
                    special_page,
                    min_role
                from
                    content_pages
                where
                    parent_page_id IS ?1
                order by page_creation_date DESC, page_id DESC
                limit ?2
        "#,
            parent_page_id,
            limit
        )
        .fetch_all(executor)
        .await?;

        Ok(content_pages)
    }

    /// Count children of `parent_page_id`, optionally filtered by `search`
    /// (matched case-insensitively against title / markdown / slug; an empty
    /// string disables the filter). Shared by the paginated `/blog` + `/projects`
    /// listings (see `web/features/listing.rs`). Applies BOTH viewer gates
    /// SQL-side so the count matches the filtered fetch and the pager stays
    /// consistent: the `datetime()`-normalized publish gate (Phase CU — non-admin
    /// sees only published, special pages exempt) and the `min_role` rank CASE
    /// (Phase DA — fail-closed, special pages NOT exempt; the CASE must stay
    /// byte-identical across the trio AND in lockstep with `min_role_rank`).
    pub async fn count_children(
        executor: impl SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
        search: &str,
        viewer: Role,
    ) -> Result<i64> {
        let viewer_is_admin = viewer == Role::Admin;
        let viewer_rank = viewer.rank() as i64;
        let row = query!(
            r#"
                select count(*) as "count!: i64"
                from content_pages
                where parent_page_id IS ?1
                  and (?2 = ''
                       or page_title    like '%' || ?2 || '%'
                       or page_markdown like '%' || ?2 || '%'
                       or page_name     like '%' || ?2 || '%')
                  and (?3
                       or special_page = 1
                       or datetime(page_creation_date) <= datetime('now'))
                  and (case when min_role is null then 0
                            when min_role = 'Registered' then 1
                            when min_role = 'Family' then 2
                            else 3 end) <= ?4
            "#,
            parent_page_id,
            search,
            viewer_is_admin,
            viewer_rank
        )
        .fetch_one(executor)
        .await?;
        Ok(row.count)
    }

    /// One page of children newest-first (the `/blog` ordering), with optional
    /// `search` + LIMIT/OFFSET. Empty `search` disables the filter. Applies the
    /// same publish gate (Phase CU) + `min_role` rank CASE (Phase DA) as
    /// `count_children` — the predicates MUST stay identical or the pager
    /// desyncs from the rows shown.
    pub async fn find_children_newest_paged(
        executor: impl SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
        search: &str,
        limit: i64,
        offset: i64,
        viewer: Role,
    ) -> Result<Vec<ContentPageDao>> {
        let viewer_is_admin = viewer == Role::Admin;
        let viewer_rank = viewer.rank() as i64;
        let content_pages: Vec<ContentPageDao> = query_as!(
            ContentPageDao,
            r#"
                select
                    page_id as "page_id!",
                    parent_page_id,
                    page_name,
                    page_title,
                    page_category,
                    page_markdown,
                    page_cover_attachment_id as "page_cover_attachment_id?",
                    page_order,
                    page_creation_date as "page_creation_date: DateTime<Utc>",
                    page_modified_date as "page_modified_date: DateTime<Utc>",
                    special_page,
                    min_role
                from content_pages
                where parent_page_id IS ?1
                  and (?2 = ''
                       or page_title    like '%' || ?2 || '%'
                       or page_markdown like '%' || ?2 || '%'
                       or page_name     like '%' || ?2 || '%')
                  and (?5
                       or special_page = 1
                       or datetime(page_creation_date) <= datetime('now'))
                  and (case when min_role is null then 0
                            when min_role = 'Registered' then 1
                            when min_role = 'Family' then 2
                            else 3 end) <= ?6
                order by page_creation_date DESC, page_id DESC
                limit ?3 offset ?4
            "#,
            parent_page_id,
            search,
            limit,
            offset,
            viewer_is_admin,
            viewer_rank
        )
        .fetch_all(executor)
        .await?;
        Ok(content_pages)
    }

    /// One page of children by manual `page_order` (the `/projects` ordering),
    /// with optional `search` + LIMIT/OFFSET. Same paired gates as
    /// `count_children` (publish + `min_role` CASE) — predicates identical by
    /// construction, see the parity tests.
    pub async fn find_children_ordered_paged(
        executor: impl SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
        search: &str,
        limit: i64,
        offset: i64,
        viewer: Role,
    ) -> Result<Vec<ContentPageDao>> {
        let viewer_is_admin = viewer == Role::Admin;
        let viewer_rank = viewer.rank() as i64;
        let content_pages: Vec<ContentPageDao> = query_as!(
            ContentPageDao,
            r#"
                select
                    page_id as "page_id!",
                    parent_page_id,
                    page_name,
                    page_title,
                    page_category,
                    page_markdown,
                    page_cover_attachment_id as "page_cover_attachment_id?",
                    page_order,
                    page_creation_date as "page_creation_date: DateTime<Utc>",
                    page_modified_date as "page_modified_date: DateTime<Utc>",
                    special_page,
                    min_role
                from content_pages
                where parent_page_id IS ?1
                  and (?2 = ''
                       or page_title    like '%' || ?2 || '%'
                       or page_markdown like '%' || ?2 || '%'
                       or page_name     like '%' || ?2 || '%')
                  and (?5
                       or special_page = 1
                       or datetime(page_creation_date) <= datetime('now'))
                  and (case when min_role is null then 0
                            when min_role = 'Registered' then 1
                            when min_role = 'Family' then 2
                            else 3 end) <= ?6
                order by page_order, page_id
                limit ?3 offset ?4
            "#,
            parent_page_id,
            search,
            limit,
            offset,
            viewer_is_admin,
            viewer_rank
        )
        .fetch_all(executor)
        .await?;
        Ok(content_pages)
    }

    pub async fn find_by_path(
        executor: impl sqlx::SqliteExecutor<'_> + Clone,
        paths: &[&str],
    ) -> Result<Vec<ContentPageDao>> {
        let mut nodes: Vec<ContentPageDao> = vec![];
        let mut current_parent_id: Option<i64> = None;

        //I suspect there is a fancy iterator pattern I should use for this long term
        for path in paths {
            let found_node = Self::find_by_name(executor.clone(), current_parent_id, path).await?;

            match found_node {
                Some(node) => {
                    current_parent_id = Some(node.page_id);

                    nodes.push(node);
                }
                None => return Ok(vec![]),
            }
        }

        Ok(nodes)
    }

    /// The human title for listings / breadcrumbs / feeds: an explicit
    /// `page_title`, else the first markdown `# H1`, else the URL slug.
    pub fn display_title(&self) -> String {
        self.page_title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| Self::first_h1(&self.page_markdown))
            .unwrap_or_else(|| self.page_name.clone())
    }

    /// True iff this page is pinned to the landing's Featured band — i.e. its
    /// `page_category` carries the reserved `featured` tag (Phase 13.8). Drives
    /// both the editor's Pin/Unpin button state and the landing query.
    pub fn is_featured(&self) -> bool {
        crate::web::util::category::is_featured(self.page_category.as_deref())
    }

    /// True iff this page's publish instant (`page_creation_date`) is in the
    /// future — SCHEDULED / a draft, not yet live. Single source of truth for the
    /// "future = hidden" rule (Phase CU): drives the Scheduled badge, the
    /// Publish-now/Unpublish button state, and `is_visible_to`. "Future" is
    /// strictly `>` now, so a page dated exactly now is already published.
    pub fn is_scheduled(&self) -> bool {
        self.page_creation_date > Utc::now()
    }

    /// The visibility gate every public read path applies — two independent
    /// clauses, and BOTH must pass:
    ///
    /// 1. **Publish** (Phase CU): `special_page` (routing redirect, exempt), OR
    ///    the viewer is an admin (previews scheduled drafts), OR the publish
    ///    instant has passed. Comparing the decoded `DateTime<Utc>` here
    ///    sidesteps the column's mixed TEXT date formats — the paginated
    ///    queries need the SQL-side `datetime()`-normalized equivalent (CU.4).
    /// 2. **Role** (Phase DA): `viewer.rank() >= min_role_rank()`. Special
    ///    pages are DELIBERATELY NOT exempt from this clause — the `library`
    ///    special page's own `min_role` is what gates its nav tab + redirect.
    pub fn is_visible_to(&self, viewer: Role) -> bool {
        let published = self.special_page || viewer == Role::Admin || !self.is_scheduled();
        published && MinRole::from_stored(self.min_role.as_deref()).is_visible_to(viewer)
    }

    /// Fail-closed decode of `min_role`, the exact Rust twin of the SQL CASE in
    /// `count_children` / both paged fetches (the parity test pins them
    /// together — including the catch-all, via the garbage-spelling row).
    /// `None` is the ONLY public spelling; a recognized gate role maps to its
    /// rank; EVERYTHING else — unknown values from a manual DB edit or a future
    /// role after a rollback, `"Admin"`, and the unsanctioned `"Anonymous"`
    /// spelling — ranks as TOP-of-ladder (Admin-only). Hiding content on a
    /// value we don't understand is recoverable; leaking it is not. The
    /// catch-all is `Role::Admin.rank()`, NOT a literal 3, so it stays the top
    /// even if the ladder is renumbered (`admin_is_the_top_rank` in roles.rs
    /// pins that invariant; the SQL CASE's `else 3` can't reference it — the
    /// parity test is what keeps the two catch-alls in lockstep).
    ///
    /// NEVER branch on the raw `min_role` string outside this fn — the DB
    /// badge / DC inheritance consumers must go through the decode, or a
    /// garbage value renders as public while gating as Admin-only.
    pub fn min_role_rank(&self) -> u8 {
        MinRole::from_stored(self.min_role.as_deref()).rank()
    }

    /// The NAV variant of the visibility gate (DB.4) — lives HERE beside
    /// `is_visible_to` so a future clause change is edited with its sibling in
    /// view, never drifting apart silently. It deliberately splits the two
    /// clauses: ROLE is viewer-aware (a Family session sees the gated
    /// `library` tab), but SCHEDULING is evaluated as-if-Anonymous — a
    /// future-dated draft tab is hidden from EVERYONE, Admin included, because
    /// the nav is unbadged and a draft tab would look live (admin previews by
    /// direct URL instead).
    pub fn is_nav_visible_to(&self, viewer: Role) -> bool {
        (self.special_page || !self.is_scheduled())
            && MinRole::from_stored(self.min_role.as_deref()).is_visible_to(viewer)
    }

    /// The badge / editor-select label for this page's visibility, derived
    /// from the fail-closed DECODE — never the raw string, so a garbage value
    /// reads as "Admin-only" instead of rendering its own text while gating as
    /// admin-only. `None` = public (no badge).
    pub fn visibility_label(&self) -> Option<&'static str> {
        MinRole::from_stored(self.min_role.as_deref()).label()
    }

    /// The post date formatted for an `<input type="datetime-local" step="1">` —
    /// the editor's backdating field. (The blog sorts + displays by this date, so
    /// setting it back-dates a Wayback-recovered post to its real slot.)
    pub fn creation_date_input(&self) -> String {
        self.page_creation_date.format("%Y-%m-%dT%H:%M:%S").to_string()
    }

    /// The publish instant as a human UTC label (e.g. `2026-07-10 13:00 UTC`),
    /// shown next to the editor's Posted field + on a scheduled page so its go-live
    /// time is stated explicitly. The datetime-local input is stored as UTC verbatim,
    /// so this reads the same wall clock with the zone made explicit (Phase CU).
    pub fn creation_date_utc_label(&self) -> String {
        self.page_creation_date.format("%Y-%m-%d %H:%M UTC").to_string()
    }

    /// First level-1 ATX heading (`# Title`) in the markdown, if any.
    fn first_h1(markdown: &str) -> Option<String> {
        markdown
            .lines()
            .map(str::trim)
            .find_map(|l| l.strip_prefix("# "))
            .map(|t| t.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;

    use super::*;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn create_basic(pool: SqlitePool) -> Result<()> {
        let mut tx = pool.begin().await?;

        ContentPageDao::create(
            &mut *tx,
            None,
            "test".to_string(),
            Some("test".to_string()),
            "test".to_string(),
            None,
        )
        .await?;

        tx.commit().await?;

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn set_cover_stamps_modified_date(pool: SqlitePool) -> Result<()> {
        let page =
            ContentPageDao::create(&pool, None, "cover-pg".to_string(), None, "body".to_string(), None)
                .await?;

        // Force modified_date into the past so the stamp is observable regardless
        // of SQLite's 1-second `datetime('now')` resolution (create() also stamps
        // it to now, so a same-second set_cover would otherwise look unchanged).
        sqlx::query(
            "UPDATE content_pages SET page_modified_date = '2000-01-01 00:00:00' WHERE page_id = ?1",
        )
        .bind(page.page_id)
        .execute(&pool)
        .await?;

        ContentPageDao::set_cover(&pool, page.page_id, None).await?;

        let row = query!(
            r#"SELECT page_modified_date as "d: DateTime<Utc>" FROM content_pages WHERE page_id = ?1"#,
            page.page_id
        )
        .fetch_one(&pool)
        .await?;
        let old: DateTime<Utc> = "2001-01-01T00:00:00Z".parse().unwrap();
        assert!(
            row.d > old,
            "set_cover must bump page_modified_date to now (got {})",
            row.d
        );
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn roundtrip(pool: SqlitePool) -> Result<()> {
        let parent_pg =
            ContentPageDao::create(&pool, None, "test".to_string(), None, "".to_string(), None)
                .await?;

        let mut leaf_pg = ContentPageDao::create(
            &pool,
            Some(parent_pg.page_id),
            "test".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;

        leaf_pg.page_category = Some("food".to_string());

        leaf_pg.update(&pool).await?;

        let found_pages = ContentPageDao::find_by_parent(&pool, Some(parent_pg.page_id)).await?;
        assert_eq!(vec![leaf_pg.clone()], found_pages);

        let found_cp = ContentPageDao::find_by_name(&pool, Some(parent_pg.page_id), "test").await?;
        assert_eq!(leaf_pg.clone(), found_cp.unwrap());

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn newest_first(pool: SqlitePool) -> Result<()> {
        let parent =
            ContentPageDao::create(&pool, None, "parent".to_string(), None, "".to_string(), None)
                .await?;

        let empty =
            ContentPageDao::find_by_parent_newest_first(&pool, Some(parent.page_id), None).await?;
        assert!(empty.is_empty());

        let a = ContentPageDao::create(
            &pool,
            Some(parent.page_id),
            "a".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;
        let b = ContentPageDao::create(
            &pool,
            Some(parent.page_id),
            "b".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;
        let c = ContentPageDao::create(
            &pool,
            Some(parent.page_id),
            "c".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;

        // CURRENT_TIMESTAMP has 1s resolution; tiebreaker is page_id DESC,
        // so insertion-order-reversed is the expected sort even when all share a timestamp.
        let all =
            ContentPageDao::find_by_parent_newest_first(&pool, Some(parent.page_id), None).await?;
        assert_eq!(vec![c.clone(), b.clone(), a.clone()], all);

        let two =
            ContentPageDao::find_by_parent_newest_first(&pool, Some(parent.page_id), Some(2))
                .await?;
        assert_eq!(vec![c.clone(), b.clone()], two);

        // posts under a different parent are not returned
        let other_parent =
            ContentPageDao::create(&pool, None, "other".to_string(), None, "".to_string(), None)
                .await?;
        ContentPageDao::create(
            &pool,
            Some(other_parent.page_id),
            "z".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;
        let still_three =
            ContentPageDao::find_by_parent_newest_first(&pool, Some(parent.page_id), None).await?;
        assert_eq!(3, still_three.len());

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn page_title_and_display_title(pool: SqlitePool) -> Result<()> {
        let mut pg = ContentPageDao::create(
            &pool,
            None,
            "my-slug".to_string(),
            None,
            "# Heading From Markdown\n\nbody".to_string(),
            None,
        )
        .await?;
        // No explicit title -> falls back to the markdown H1.
        assert_eq!(pg.page_title, None);
        assert_eq!(pg.display_title(), "Heading From Markdown");

        // Explicit title wins, and round-trips through the DB.
        pg.page_title = Some("Explicit Title".to_string());
        pg.update(&pool).await?;
        let found = ContentPageDao::find_by_name(&pool, None, "my-slug")
            .await?
            .unwrap();
        assert_eq!(found.page_title.as_deref(), Some("Explicit Title"));
        assert_eq!(found.display_title(), "Explicit Title");

        // No title + no H1 -> falls back to the slug.
        let plain = ContentPageDao::create(
            &pool,
            None,
            "plain".to_string(),
            None,
            "no heading here".to_string(),
            None,
        )
        .await?;
        assert_eq!(plain.display_title(), "plain");

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn find_path(pool: SqlitePool) -> Result<()> {
        let root =
            ContentPageDao::create(&pool, None, "root".to_string(), None, "".to_string(), None)
                .await?;

        let leaf = ContentPageDao::create(
            &pool,
            Some(root.page_id),
            "leaf".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;

        let deep_leaf = ContentPageDao::create(
            &pool,
            Some(leaf.page_id),
            "deep_leaf".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;

        assert_eq!(
            vec![root.clone()],
            ContentPageDao::find_by_path(&pool, &["root"]).await?
        );

        assert_eq!(
            vec![root, leaf, deep_leaf],
            ContentPageDao::find_by_path(&pool, &["root", "leaf", "deep_leaf"]).await?
        );

        assert_eq!(
            0,
            ContentPageDao::find_by_path(&pool, &["random"])
                .await?
                .len()
        );

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn set_order_drives_find_by_parent(pool: SqlitePool) -> Result<()> {
        let parent =
            ContentPageDao::create(&pool, None, "parent".to_string(), None, "".to_string(), None)
                .await?;
        let a = ContentPageDao::create(
            &pool,
            Some(parent.page_id),
            "a".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;
        let b = ContentPageDao::create(
            &pool,
            Some(parent.page_id),
            "b".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;
        let c = ContentPageDao::create(
            &pool,
            Some(parent.page_id),
            "c".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;

        // Reverse the visual order via set_order; find_by_parent reflects it.
        ContentPageDao::set_order(&pool, c.page_id, 0).await?;
        ContentPageDao::set_order(&pool, b.page_id, 1).await?;
        ContentPageDao::set_order(&pool, a.page_id, 2).await?;

        let ordered: Vec<i64> = ContentPageDao::find_by_parent(&pool, Some(parent.page_id))
            .await?
            .into_iter()
            .map(|p| p.page_id)
            .collect();
        assert_eq!(vec![c.page_id, b.page_id, a.page_id], ordered);

        Ok(())
    }

    fn page_dated(creation: DateTime<Utc>, special_page: bool) -> ContentPageDao {
        ContentPageDao {
            page_id: 1,
            parent_page_id: None,
            page_name: "p".to_string(),
            page_title: None,
            page_category: None,
            page_markdown: String::new(),
            page_cover_attachment_id: None,
            page_order: 0,
            page_creation_date: creation,
            page_modified_date: creation,
            special_page,
            min_role: None,
        }
    }

    #[test]
    fn scheduled_and_visibility_gate() {
        // Fixed far past/future so the boundary is deterministic (no now() flake).
        let past: DateTime<Utc> = "2000-01-01T00:00:00Z".parse().unwrap();
        let future: DateTime<Utc> = "2999-01-01T00:00:00Z".parse().unwrap();

        let past_pg = page_dated(past, false);
        let future_pg = page_dated(future, false);
        let future_special = page_dated(future, true);

        // Scheduled iff dated in the future.
        assert!(!past_pg.is_scheduled());
        assert!(future_pg.is_scheduled());

        // Non-admin: past page visible, future page hidden.
        assert!(past_pg.is_visible_to(Role::Anonymous));
        assert!(!future_pg.is_visible_to(Role::Anonymous));

        // Admin sees the scheduled page; special pages are exempt from the
        // SCHEDULING clause.
        assert!(future_pg.is_visible_to(Role::Admin));
        assert!(future_special.is_visible_to(Role::Anonymous));
    }

    /// DA.2: the role clause. NULL is public; each gate role admits its rank and
    /// above; special pages are NOT exempt from the role clause (the `library`
    /// special page's own min_role gates its nav tab); unknown values and the
    /// unsanctioned "Anonymous" spelling fail CLOSED to Admin-only.
    #[test]
    fn min_role_gate_is_fail_closed() {
        let past: DateTime<Utc> = "2000-01-01T00:00:00Z".parse().unwrap();
        let with_min = |min_role: Option<&str>, special| {
            let mut p = page_dated(past, special);
            p.min_role = min_role.map(str::to_string);
            p
        };

        // NULL = public.
        assert!(with_min(None, false).is_visible_to(Role::Anonymous));

        // Family-gated: Family + Admin in, Anonymous + Registered out.
        let fam = with_min(Some("Family"), false);
        assert!(!fam.is_visible_to(Role::Anonymous));
        assert!(!fam.is_visible_to(Role::Registered));
        assert!(fam.is_visible_to(Role::Family));
        assert!(fam.is_visible_to(Role::Admin));

        // A gated SPECIAL page is still gated — the exemption covers scheduling only.
        let gated_special = with_min(Some("Family"), true);
        assert!(!gated_special.is_visible_to(Role::Anonymous));
        assert!(gated_special.is_visible_to(Role::Family));

        // Fail-closed: garbage and the unsanctioned "Anonymous" spelling are
        // Admin-only, never public.
        for bad in ["Garbage", "Anonymous", "family", ""] {
            let p = with_min(Some(bad), false);
            assert!(!p.is_visible_to(Role::Family), "{bad:?} must fail closed");
            assert!(p.is_visible_to(Role::Admin));
            assert_eq!(p.min_role_rank(), 3);
        }
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn set_creation_date_stamps_and_schedules(pool: SqlitePool) -> Result<()> {
        let page = ContentPageDao::create(
            &pool,
            None,
            "sched".to_string(),
            None,
            "body".to_string(),
            None,
        )
        .await?;
        // Force modified_date into the past so the stamp is observable.
        sqlx::query(
            "UPDATE content_pages SET page_modified_date = '2000-01-01 00:00:00' WHERE page_id = ?1",
        )
        .bind(page.page_id)
        .execute(&pool)
        .await?;

        let future: DateTime<Utc> = "2999-01-01T00:00:00Z".parse().unwrap();
        ContentPageDao::set_creation_date(&pool, page.page_id, future).await?;

        let found = ContentPageDao::find_by_id(&pool, page.page_id)
            .await?
            .unwrap();
        assert_eq!(found.page_creation_date, future);
        assert!(
            found.is_scheduled(),
            "a future creation date must read as scheduled"
        );
        let old: DateTime<Utc> = "2001-01-01T00:00:00Z".parse().unwrap();
        assert!(
            found.page_modified_date > old,
            "set_creation_date must stamp page_modified_date"
        );
        Ok(())
    }

    /// DA.3: the SQL CASE (count + both paged fetches) and the Rust
    /// `min_role_rank()`/`rank()` pair MUST agree for NULL, every Role variant
    /// name, and garbage — driven by `Role::iter()` so a future variant fails
    /// here until BOTH ladders learn it. Also pins count↔fetch parity per viewer.
    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn min_role_sql_case_matches_rust_rank(pool: SqlitePool) -> Result<()> {
        use strum::IntoEnumIterator;
        use crate::db::dao::roles::Role;

        let parent =
            ContentPageDao::create(&pool, None, "parent".to_string(), None, "".to_string(), None)
                .await?;
        let mut spellings: Vec<Option<String>> = vec![None, Some("Garbage".to_string())];
        spellings.extend(Role::iter().map(|r| Some(r.to_string())));
        for (i, s) in spellings.iter().enumerate() {
            let pg = ContentPageDao::create(
                &pool,
                Some(parent.page_id),
                format!("c{i}"),
                None,
                "".to_string(),
                None,
            )
            .await?;
            sqlx::query("UPDATE content_pages SET min_role = ?1 WHERE page_id = ?2")
                .bind(s)
                .bind(pg.page_id)
                .execute(&pool)
                .await?;
        }

        let pid = Some(parent.page_id);
        let all = ContentPageDao::find_by_parent(&pool, pid).await?;
        assert_eq!(all.len(), spellings.len());

        for viewer in Role::iter() {
            // Compare row SETS, not cardinalities — a CASE drift between the
            // count query and a paged fetch that happened to preserve counts
            // would otherwise slip through.
            let mut rust_ids: Vec<i64> = all
                .iter()
                .filter(|p| p.is_visible_to(viewer))
                .map(|p| p.page_id)
                .collect();
            rust_ids.sort_unstable();
            let sql_count = ContentPageDao::count_children(&pool, pid, "", viewer).await?;
            assert_eq!(
                sql_count,
                rust_ids.len() as i64,
                "SQL CASE vs Rust rank drifted for viewer {viewer}"
            );
            let mut newest_ids: Vec<i64> =
                ContentPageDao::find_children_newest_paged(&pool, pid, "", 100, 0, viewer)
                    .await?
                    .iter()
                    .map(|p| p.page_id)
                    .collect();
            newest_ids.sort_unstable();
            assert_eq!(newest_ids, rust_ids, "newest fetch set drifted for {viewer}");
            let mut ordered_ids: Vec<i64> =
                ContentPageDao::find_children_ordered_paged(&pool, pid, "", 100, 0, viewer)
                    .await?
                    .iter()
                    .map(|p| p.page_id)
                    .collect();
            ordered_ids.sort_unstable();
            assert_eq!(ordered_ids, rust_ids, "ordered fetch set drifted for {viewer}");
        }

        // Pin the ABSOLUTE ladder too, not just cross-parity: NULL only /
        // +Registered / +Family / Admin sees all six ('Admin', the unsanctioned
        // 'Anonymous' spelling, and 'Garbage' all rank as Admin-only).
        assert_eq!(1, ContentPageDao::count_children(&pool, pid, "", Role::Anonymous).await?);
        assert_eq!(2, ContentPageDao::count_children(&pool, pid, "", Role::Registered).await?);
        assert_eq!(3, ContentPageDao::count_children(&pool, pid, "", Role::Family).await?);
        assert_eq!(6, ContentPageDao::count_children(&pool, pid, "", Role::Admin).await?);

        // POSITIVE per-variant pin — the teeth for a FUTURE variant. The
        // cross-parity loop above cannot catch an unlearned role: both ladders
        // fail closed to Admin-only and still agree. But a row gated at exactly
        // R must be VISIBLE to viewer R (in Rust AND in the SQL fetch) — an
        // unlearned variant fails this until both the CASE and min_role_rank()
        // are taught its true rank. (Anonymous is exempt: its spelling is
        // unsanctioned and deliberately Admin-only.)
        for r in Role::iter().filter(|r| *r != Role::Anonymous) {
            let spelling = r.to_string();
            let row = all
                .iter()
                .find(|p| p.min_role.as_deref() == Some(spelling.as_str()))
                .expect("one child per role spelling was seeded");
            assert!(
                row.is_visible_to(r),
                "a row gated at exactly {r} must admit viewer {r} (Rust)"
            );
            let fetched =
                ContentPageDao::find_children_newest_paged(&pool, pid, "", 100, 0, r).await?;
            assert!(
                fetched.iter().any(|p| p.page_id == row.page_id),
                "a row gated at exactly {r} must admit viewer {r} (SQL CASE)"
            );
        }
        Ok(())
    }

    /// CU.4 core risk: the count and the paged fetch MUST apply the same publish
    /// gate, or the pager (total_pages/offset) desyncs from the rows shown.
    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn paginated_gate_hides_future_from_non_admin(pool: SqlitePool) -> Result<()> {
        let parent =
            ContentPageDao::create(&pool, None, "parent".to_string(), None, "".to_string(), None)
                .await?;
        for slug in ["pub-a", "pub-b"] {
            ContentPageDao::create(
                &pool,
                Some(parent.page_id),
                slug.to_string(),
                None,
                "".to_string(),
                None,
            )
            .await?;
        }
        let future = ContentPageDao::create(
            &pool,
            Some(parent.page_id),
            "future".to_string(),
            None,
            "".to_string(),
            None,
        )
        .await?;
        let far: DateTime<Utc> = "2999-01-01T00:00:00Z".parse().unwrap();
        ContentPageDao::set_creation_date(&pool, future.page_id, far).await?;

        let pid = Some(parent.page_id);
        // Non-admin: count AND both paged fetches exclude the future child (parity).
        assert_eq!(2, ContentPageDao::count_children(&pool, pid, "", Role::Anonymous).await?);
        assert_eq!(
            2,
            ContentPageDao::find_children_newest_paged(&pool, pid, "", 10, 0, Role::Anonymous)
                .await?
                .len()
        );
        assert_eq!(
            2,
            ContentPageDao::find_children_ordered_paged(&pool, pid, "", 10, 0, Role::Anonymous)
                .await?
                .len()
        );
        // Admin: sees all three, count + rows agree.
        assert_eq!(3, ContentPageDao::count_children(&pool, pid, "", Role::Admin).await?);
        assert_eq!(
            3,
            ContentPageDao::find_children_newest_paged(&pool, pid, "", 10, 0, Role::Admin)
                .await?
                .len()
        );
        assert_eq!(
            3,
            ContentPageDao::find_children_ordered_paged(&pool, pid, "", 10, 0, Role::Admin)
                .await?
                .len()
        );
        Ok(())
    }
}
