use anyhow::Result;
use sqlx::{prelude::FromRow, query, query_as, SqliteExecutor};

#[derive(Clone, Debug, FromRow, PartialEq)]
pub struct AttachmentDao {
    pub attachment_id: i64,
    pub page_id: i64,
    pub attachment_name: String,
    pub mime_type: String,
    pub attachment_content: Vec<u8>,
}

impl AttachmentDao {
    pub async fn create(
        executor: impl SqliteExecutor<'_>,
        page_id: i64,
        attachment_name: String,
        mime_type: String,
        attachment_content: Vec<u8>,
    ) -> Result<AttachmentDao> {
        let result = query!(
            r#"
        INSERT INTO attachments (
            page_id,
            attachment_name,
            mime_type,
            attachment_content
        ) VALUES (
            ?1,
            ?2,
            ?3,
            ?4
        ) RETURNING 
            attachment_id
        "#,
            page_id,
            attachment_name,
            mime_type,
            attachment_content
        )
        .execute(executor)
        .await?;

        Ok(AttachmentDao {
            attachment_id: result.attachment_id,
            page_id,
            attachment_name,
            mime_type,
            attachment_content,
        })
    }

    pub async fn delete(&self, executor: impl SqliteExecutor<'_>) -> Result<()> {
        query!(
            r#"
            DELETE FROM attachments
            WHERE 
                page_id = ?1
                and attachment_name = ?2
            "#,
            self.page_id,
            self.attachment_name
        )
        .execute(executor)
        .await?;

        Ok(())
    }

    pub async fn update(&self, executor: impl SqliteExecutor<'_>) -> Result<()> {
        let result = query!(
            r#"
        UPDATE attachments
        SET 
            attachment_name = ?1,
            mime_type = ?2,
            attachment_content = ?3
        WHERE
            attachment_id = ?7
        "#,
            self.attachment_name,
            self.mime_type,
            self.attachment_content,
            self.attachment_id
        )
        .execute(executor)
        .await?;

        Ok(())
    }

    pub async fn find_attachments_by_page_id(
        executor: impl SqliteExecutor<'_>,
        page_id: i64,
    ) -> Result<Vec<AttachmentDao>> {
        let attachments = query_as(
            r#"
        select 
            attachment_id,
            page_id,
            attachment_name,
            mime_type,
            attachment_content,
        from
            attachments
        where page_id = ?1
        order by attachment_name
        "#,
        )
        .bind(page_id)
        .fetch_all(executor)
        .await?;

        Ok(attachments)
    }

    pub async fn find_attachment_by_id(
        executor: impl SqliteExecutor<'_>,
        attachment_id: i64,
    ) -> Result<Option<AttachmentDao>> {
        let attachment = query_as(
            r#"
        select 
            attachment_id,
            page_id,
            attachment_name,
            mime_type,
            attachment_content,
        from
            attachments
        where attachment_id = ?1
        order by attachment_name
        "#,
        )
        .bind(attachment_id)
        .fetch_optional(executor)
        .await?;

        Ok(attachment)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::dao::content_pages::ContentPageDao;
    use sqlx::SqlitePool;

    use super::*;

    #[sqlx::test]
    async fn roundtrip(pool: SqlitePool) -> Result<()> {
        let cp =
            ContentPageDao::create(&pool, None, "test".to_string(), None, "".to_string(), None)
                .await?;

        let mut attach = AttachmentDao::create(
            &pool,
            cp.page_id,
            "image.jpg".to_string(),
            "application/jpeg".to_string(),
            "test string".as_bytes().to_vec(),
        )
        .await?;

        attach.mime_type = "fake".to_string();
        attach.update(&pool).await?;

        let found_attach =
            AttachmentDao::find_attachment_by_id(&pool, attach.attachment_id).await?;
        assert_eq!(attach, found_attach.unwrap());

        let found_attach_page =
            AttachmentDao::find_attachments_by_page_id(&pool, cp.page_id).await?;
        assert_eq!(vec![attach], found_attach_page);

        Ok(())
    }
}
