use crate::db::dao::content_pages::ContentPageDao;
use anyhow::Result;
use sqlx::SqliteExecutor;

/// Navigation tabs plus the active nav key. `tabs` is the content pages as
/// `(name, is_active)`; `active` is the raw active key so the hardcoded admin
/// tabs (e.g. `"admin"`) can mark themselves active by the same mechanism — the
/// page being rendered decides what's active, not a per-tab hardcode.
pub struct TopBar {
    pub tabs: Vec<(String, bool)>,
    pub active: String,
}

impl TopBar {
    pub async fn create(executor: impl SqliteExecutor<'_>, active_page: &str) -> Result<Self> {
        let tabs = ContentPageDao::find_by_parent(executor, None)
            .await?
            .into_iter()
            // Hide future-dated (scheduled/draft) top-level pages from the global
            // nav — they'd otherwise show as a live tab site-wide before their
            // publish instant (Phase CU). Special pages (blog/projects/resume/login)
            // are exempt via is_visible_to; admin reaches a scheduled top-level page
            // through its direct URL or the admin Manage Pages list, not the nav.
            .filter(|cpd| cpd.is_visible_to(false))
            .map(|cpd| cpd.page_name)
            .map(|name| {
                let m = name == active_page;
                (name, m)
            })
            .collect();
        Ok(TopBar {
            tabs,
            active: active_page.to_string(),
        })
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
        for (title, active) in &tb.tabs {
            if title == "first" {
                assert!(*active);
            } else {
                assert!(!*active);
            }
        }
        assert!(!tb.tabs.is_empty());

        let tb = TopBar::create(&pool, "not here").await?;
        for (_, active) in &tb.tabs {
            assert!(!*active);
        }
        assert!(!tb.tabs.is_empty());

        Ok(())
    }
}
