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
            page_category = ?3,
            page_markdown = ?4,
            page_cover_attachment_id = ?5,
            page_order = ?6,
            page_modified_date = datetime('now', 'utc')
        WHERE
            page_id = ?7
        RETURNING
            page_modified_date as "page_modified_date: DateTime<Utc>"
        "#,
            self.parent_page_id,
            self.page_name,
            self.page_category,
            self.page_markdown,
            self.page_cover_attachment_id,
            self.page_order,
            self.page_id
        )
        .fetch_one(executor)
        .await?;

        self.page_modified_date = result.page_modified_date;

        Ok(())
    }

    pub async fn find_by_parent(
        executor: impl SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
    ) -> Result<Vec<ContentPageDao>> {
        let content_pages: Vec<ContentPageDao> = query_as(
            r#"
                select 
                    page_id,
                    parent_page_id,
                    page_name,
                    page_category,
                    page_markdown,
                    page_cover_attachment_id,
                    page_order,
                    page_creation_date,
                    page_modified_date,
                    special_page
                from
                    content_pages
                where
                    parent_page_id IS ?1
                order by page_order
        "#,
        )
        .bind(parent_page_id)
        .fetch_all(executor)
        .await?;

        Ok(content_pages)
    }

    pub async fn find_by_name(
        executor: impl sqlx::SqliteExecutor<'_>,
        parent_page_id: Option<i64>,
        page_name: &str,
    ) -> Result<Option<ContentPageDao>> {
        let content_page: Option<ContentPageDao> = query_as(
            r#"
                select 
                    page_id,
                    parent_page_id,
                    page_name,
                    page_category,
                    page_markdown,
                    page_cover_attachment_id,
                    page_order,
                    page_creation_date,
                    page_modified_date,
                    special_page
                from
                    content_pages
                where
                    parent_page_id IS ?1
                    and page_name = ?2
                order by page_order
        "#,
        )
        .bind(parent_page_id)
        .bind(page_name)
        .fetch_optional(executor)
        .await?;

        Ok(content_page)
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
                    let special_page = node.special_page;
                    current_parent_id = Some(node.page_id);

                    nodes.push(node);

                    if special_page {
                        break;
                    }
                }
                None => return Ok(vec![]),
            }
        }

        Ok(nodes)
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
        let mut parent_pg =
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
}
