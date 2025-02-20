use crate::db::dao::content_pages::ContentPageDao;
use anyhow::Result;
use sqlx::SqliteExecutor;

/// A wrapper type used to pass the navigation tabs and which one is active
pub struct TopBar(pub Vec<(String, bool)>);

impl TopBar {
    pub async fn create(executor: impl SqliteExecutor<'_>, active_page: &str) -> Result<Self> {
        let pages = ContentPageDao::find_by_parent(executor, None)
            .await?
            .into_iter()
            .map(|cpd| cpd.page_name)
            .map(|name| {
                let m = name == active_page;
                (name, m)
            })
            .collect();
        Ok(TopBar(pages))
    }
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;

    use super::*;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn activate(pool: SqlitePool) -> Result<()> {
        ContentPageDao::create(
            &pool,
            None,
            "first".to_string(),
            None,
            "first".to_string(),
            None,
        )
        .await?;
        ContentPageDao::create(
            &pool,
            None,
            "second".to_string(),
            None,
            "second".to_string(),
            None,
        )
        .await?;

        let tb = TopBar::create(&pool, "first").await?;
        for (title, active) in &tb.0 {
            if title == "first" {
                assert!(*active);
            } else {
                assert!(!*active);
            }
        }
        assert!(!tb.0.is_empty());

        let tb = TopBar::create(&pool, "not here").await?;
        for (_, active) in &tb.0 {
            assert!(!*active);
        }
        assert!(!tb.0.is_empty());

        Ok(())
    }
}
