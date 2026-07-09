use crate::db::dao::content_pages::ContentPageDao;
use crate::db::dao::roles::Role;
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
    pub async fn create(
        executor: impl SqliteExecutor<'_>,
        active_page: &str,
        viewer: Role,
    ) -> Result<Self> {
        let tabs = ContentPageDao::find_by_parent(executor, None)
            .await?
            .into_iter()
            // Role-aware nav (DB.4): role is viewer-aware, scheduling is
            // hidden-from-everyone — the split semantics live in the DAO
            // beside is_visible_to (see is_nav_visible_to's doc).
            .filter(|cpd| cpd.is_nav_visible_to(viewer))
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

        let tb = TopBar::create(&pool, "first", Role::Anonymous).await?;
        for (title, active) in &tb.tabs {
            if title == "first" {
                assert!(*active);
            } else {
                assert!(!*active);
            }
        }
        assert!(!tb.tabs.is_empty());

        let tb = TopBar::create(&pool, "not here", Role::Anonymous).await?;
        for (_, active) in &tb.tabs {
            assert!(!*active);
        }
        assert!(!tb.tabs.is_empty());

        Ok(())
    }

    /// DB.4: the nav is role-aware — a gated tab shows for viewers whose rank
    /// admits it (Family/Admin here) and stays hidden below (Anonymous,
    /// Registered). This is the seam the DE `library` special row rides.
    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn gated_tab_is_role_aware(pool: SqlitePool) -> Result<()> {
        ContentPageDao::create(&pool, None, "open".to_string(), None, "".to_string(), None)
            .await?;
        ContentPageDao::create(&pool, None, "kin".to_string(), None, "".to_string(), None)
            .await?;
        sqlx::query("UPDATE content_pages SET min_role = 'Family' WHERE page_name = 'kin'")
            .execute(&pool)
            .await?;

        let has_kin = |tb: &TopBar| tb.tabs.iter().any(|(name, _)| name == "kin");
        for (viewer, expect) in [
            (Role::Anonymous, false),
            (Role::Registered, false),
            (Role::Family, true),
            (Role::Admin, true),
        ] {
            let tb = TopBar::create(&pool, "open", viewer).await?;
            assert_eq!(has_kin(&tb), expect, "viewer {viewer}");
            assert!(has_kin(&tb) || tb.tabs.iter().any(|(n, _)| n == "open"));
        }
        Ok(())
    }

    /// Scheduling stays UNCONDITIONAL in the nav (the pre-DB behavior): a
    /// future-dated draft tab is hidden from EVERYONE — Admin included, since
    /// the nav is unbadged and a draft tab would look live. Only the ROLE
    /// clause is viewer-aware.
    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn scheduled_tab_hidden_from_everyone(pool: SqlitePool) -> Result<()> {
        ContentPageDao::create(&pool, None, "live".to_string(), None, "".to_string(), None)
            .await?;
        ContentPageDao::create(&pool, None, "draft".to_string(), None, "".to_string(), None)
            .await?;
        sqlx::query(
            "UPDATE content_pages SET page_creation_date = '2999-01-01 00:00:00' WHERE page_name = 'draft'",
        )
        .execute(&pool)
        .await?;

        use strum::IntoEnumIterator;
        for viewer in Role::iter() {
            let tb = TopBar::create(&pool, "live", viewer).await?;
            assert!(
                !tb.tabs.iter().any(|(n, _)| n == "draft"),
                "a scheduled draft tab must be hidden from {viewer} too"
            );
            assert!(tb.tabs.iter().any(|(n, _)| n == "live"));
        }
        Ok(())
    }
}
