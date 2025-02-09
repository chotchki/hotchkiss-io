use anyhow::Result;
use sqlx::{prelude::FromRow, query, query_as, SqlitePool};

#[derive(Clone, Debug, FromRow, PartialEq)]
pub struct AttachmentDao {
    pub parent_page_name: String,
    pub attachment_name: String,
    pub mime_type: String,
    pub attachment_content: Vec<u8>,
}

impl AttachmentDao {
    pub async fn delete(&self, pool: &SqlitePool) -> Result<()> {
        query!(
            r#"
            DELETE FROM attachments
            WHERE 
                parent_page_name = ?1
                and attachment_name = ?2
            "#,
            self.parent_page_name,
            self.attachment_name
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn save(&self, executor: impl sqlx::SqliteExecutor<'_>) -> Result<()> {
        query!(
            r#"
        INSERT INTO attachments (
            parent_page_name,
            attachment_name,
            mime_type,
            attachment_content
        ) VALUES (
            ?1,
            ?2,
            ?3,
            ?4
        ) 
        ON CONFLICT(parent_page_name, attachment_name) 
        DO UPDATE 
            SET mime_type = ?3,
                attachment_content = ?4
        "#,
            self.parent_page_name,
            self.attachment_name,
            self.mime_type,
            self.attachment_content
        )
        .execute(executor)
        .await?;

        Ok(())
    }

    pub async fn find_attachment_titles_by_parent(
        pool: &SqlitePool,
        parent_name: &str,
    ) -> Result<Vec<String>> {
        let title_recs = query!(
            r#"
        select 
            attachment_name
        from
            attachments
        where parent_page_name = ?1
        order by attachment_name
        "#,
            parent_name
        )
        .fetch_all(pool)
        .await?;

        let titles: Vec<String> = title_recs.into_iter().map(|r| r.attachment_name).collect();

        Ok(titles)
    }

    pub async fn find_attachment(
        pool: &SqlitePool,
        parent_name: &str,
        attachment_name: &str,
    ) -> Result<Option<AttachmentDao>> {
        let attachment = query_as!(
            AttachmentDao,
            r#"
        select 
            parent_page_name,
            attachment_name,
            mime_type,
            attachment_content
        from
            attachments
        where parent_page_name = ?1
        and attachment_name = ?2
        "#,
            parent_name,
            attachment_name
        )
        .fetch_optional(pool)
        .await?;

        Ok(attachment)
    }
}
