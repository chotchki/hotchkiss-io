use anyhow::Result;
use sqlx::{
    prelude::FromRow,
    query, query_as,
    types::chrono::{self, DateTime, Utc},
    SqliteExecutor,
};

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
            page_modified_date = datetime('now', 'utc')
        WHERE
            page_id = ?9
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
            self.page_id
        )
        .fetch_one(executor)
        .await?;

        self.page_modified_date = result.page_modified_date;

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
                    special_page
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
                    special_page
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
                    special_page
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
                    special_page
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

    /// The post date formatted for an `<input type="datetime-local" step="1">` —
    /// the editor's backdating field. (The blog sorts + displays by this date, so
    /// setting it back-dates a Wayback-recovered post to its real slot.)
    pub fn creation_date_input(&self) -> String {
        self.page_creation_date.format("%Y-%m-%dT%H:%M:%S").to_string()
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
}
