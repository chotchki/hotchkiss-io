use anyhow::Result;
use sqlx::{prelude::FromRow, query, query_as, SqliteExecutor, SqlitePool};
use tracing::debug;

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
            page_creation_date, 
            page_modified_date,
            special_page
        "#,
            parent_page_id,
            page_name,
            page_category,
            page_markdown,
            page_cover_attachment_id,
            page_order
        )
        .execute(executor)
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
            WHERE page_name = ?1
            and special_page = false
            "#,
            self.page_name
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
            page_modified_date = datetime('now', 'utc'),
        WHERE
            page_id = ?7
        RETURNING
            page_modified_date
        "#,
            self.parent_page_id,
            self.page_name,
            self.page_category,
            self.page_markdown,
            self.page_cover_attachment_id,
            self.page_order,
            self.page_id
        )
        .execute(executor)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn roundtrip(pool: SqlitePool) -> Result<()> {
        let mut cp =
            ContentPageDao::create(&pool, None, "test".to_string(), None, "".to_string(), None)
                .await?;

        cp.page_category = Some("food".to_string());

        cp.update(&pool).await?;

        let found_pages = ContentPageDao::find_by_parent(&pool, None).await?;
        assert_eq!(vec![cp.clone()], found_pages);

        let found_cp = ContentPageDao::find_by_name(&pool, None, "test").await?;
        assert_eq!(cp, found_cp.unwrap());

        Ok(())
    }
}
