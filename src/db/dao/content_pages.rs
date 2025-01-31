use anyhow::Result;
use serde_json::json;
use sqlx::Error::ColumnDecode;
use sqlx::{prelude::FromRow, query, query_as, sqlite::SqliteRow, Row, SqlitePool};
use tower_sessions::cookie::Key;
use tracing::debug;

#[derive(Clone, Debug, PartialEq)]
pub struct ContentPage {
    pub page_name: String,
    pub page_markdown: String,
    pub page_order: i64,
    pub special_page: bool,
}

impl FromRow<'_, SqliteRow> for ContentPage {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        debug!("Decoding using FromRow");

        Ok(ContentPage {
            page_name: row.try_get("page_name")?,
            page_markdown: row.try_get("page_markdown")?,
            page_order: row.try_get("page_order")?,
            special_page: match row.try_get::<i64, &str>("special_page")? {
                0 => false,
                _ => true,
            },
        })
    }
}

pub async fn save(pool: &SqlitePool, cp: &ContentPage) -> Result<()> {
    debug!("Saving Content Page {}", cp.page_name);

    query!(
        r#"
        INSERT INTO content_pages (
            page_name,
            page_markdown,
            page_order,
            special_page
        ) VALUES (
            ?1,
            ?2,
            ?3,
            ?4
        ) 
        ON CONFLICT(page_name) 
        DO UPDATE 
            SET page_markdown = ?2,
                page_order = ?3,
                special_page = ?4
        "#,
        cp.page_name,
        cp.page_markdown,
        cp.page_order,
        cp.special_page
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn find_page_titles(pool: &SqlitePool) -> Result<Vec<String>> {
    let title_recs = query!(
        r#"
        select 
            page_name
        from
            content_pages
        order by page_order
        "#
    )
    .fetch_all(pool)
    .await?;

    let titles: Vec<String> = title_recs.into_iter().map(|r| r.page_name).collect();

    Ok(titles)
}

pub async fn get_page_by_name(pool: &SqlitePool, page_name: &str) -> Result<ContentPage> {
    Ok(query_as(
        r#"
        select
            page_name,
            page_markdown,
            page_order,
            special_page
        from
            content_pages
        where
            page_name = ?1
    "#,
    )
    .bind(page_name)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn roundtrip(pool: SqlitePool) -> Result<()> {
        let cp = ContentPage {
            page_name: "test".to_string(),
            page_markdown: "test content".to_string(),
            page_order: 1,
            special_page: true,
        };

        save(&pool, &cp).await?;

        let found_cp = get_page_by_name(&pool, "test").await?;
        assert_eq!(cp, found_cp);

        let page_titles = find_page_titles(&pool).await?;

        assert_eq!(vec!["test".to_string()], page_titles);

        Ok(())
    }
}
