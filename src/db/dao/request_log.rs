use anyhow::Result;
use sqlx::{
    prelude::FromRow,
    query, query_as,
    types::chrono::{DateTime, Utc},
    SqliteExecutor,
};

/// A persisted request observation, projected for the "recent requests" view.
/// `ts` is stamped by SQLite (`CURRENT_TIMESTAMP`, UTC) on insert; `ip` /
/// `user_agent` are best-effort and may be null. (`id` and `referer` are also
/// columns on the table — `referer` is recorded but not surfaced here yet.)
#[derive(Clone, Debug, FromRow)]
pub struct RequestLogDao {
    pub ts: DateTime<Utc>,
    pub method: String,
    pub path: String,
    pub status: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

/// A request as observed by the logging middleware, before it gets an id / timestamp.
#[derive(Clone, Debug)]
pub struct NewRequestLog {
    pub method: String,
    pub path: String,
    pub status: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PathCount {
    pub path: String,
    pub count: i64,
}

#[derive(Clone, Debug)]
pub struct DayCount {
    pub day: String,
    pub count: i64,
}

#[derive(Clone, Debug)]
pub struct UserAgentCount {
    pub user_agent: Option<String>,
    pub count: i64,
}

fn window(days: i64) -> String {
    // SQLite datetime modifier — `datetime('now', '-7 days')`.
    format!("-{} days", days.max(0))
}

impl RequestLogDao {
    pub async fn insert(executor: impl SqliteExecutor<'_>, new: &NewRequestLog) -> Result<()> {
        query!(
            r#"
            INSERT INTO request_log (method, path, status, ip, user_agent, referer)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            new.method,
            new.path,
            new.status,
            new.ip,
            new.user_agent,
            new.referer,
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    pub async fn recent(
        executor: impl SqliteExecutor<'_>,
        limit: i64,
    ) -> Result<Vec<RequestLogDao>> {
        Ok(query_as!(
            RequestLogDao,
            r#"
            SELECT
                ts as "ts: DateTime<Utc>",
                method,
                path,
                status,
                ip,
                user_agent
            FROM request_log
            ORDER BY id DESC
            LIMIT ?1
            "#,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    pub async fn count_since(executor: impl SqliteExecutor<'_>, since_days: i64) -> Result<i64> {
        let w = window(since_days);
        Ok(query!(
            r#"SELECT COUNT(*) as "count!: i64" FROM request_log WHERE ts >= datetime('now', ?1)"#,
            w
        )
        .fetch_one(executor)
        .await?
        .count)
    }

    pub async fn distinct_ip_count(
        executor: impl SqliteExecutor<'_>,
        since_days: i64,
    ) -> Result<i64> {
        let w = window(since_days);
        Ok(query!(
            r#"
            SELECT COUNT(DISTINCT ip) as "count!: i64"
            FROM request_log
            WHERE ts >= datetime('now', ?1) AND ip IS NOT NULL
            "#,
            w
        )
        .fetch_one(executor)
        .await?
        .count)
    }

    pub async fn count_by_user_agent(
        executor: impl SqliteExecutor<'_>,
        since_days: i64,
        limit: i64,
    ) -> Result<Vec<UserAgentCount>> {
        let w = window(since_days);
        Ok(query_as!(
            UserAgentCount,
            r#"
            SELECT user_agent, COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= datetime('now', ?1)
            GROUP BY user_agent
            ORDER BY COUNT(*) DESC, user_agent ASC
            LIMIT ?2
            "#,
            w,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    pub async fn count_by_day(
        executor: impl SqliteExecutor<'_>,
        since_days: i64,
    ) -> Result<Vec<DayCount>> {
        let w = window(since_days);
        Ok(query_as!(
            DayCount,
            r#"
            SELECT substr(ts, 1, 10) as "day!: String", COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= datetime('now', ?1)
            GROUP BY substr(ts, 1, 10)
            ORDER BY substr(ts, 1, 10) ASC
            "#,
            w
        )
        .fetch_all(executor)
        .await?)
    }

    /// Unique visitors (distinct IP) per day over the window. NULL-ip rows are
    /// excluded — they can't be attributed to a visitor.
    pub async fn distinct_ip_by_day(
        executor: impl SqliteExecutor<'_>,
        since_days: i64,
    ) -> Result<Vec<DayCount>> {
        let w = window(since_days);
        Ok(query_as!(
            DayCount,
            r#"
            SELECT substr(ts, 1, 10) as "day!: String", COUNT(DISTINCT ip) as "count!: i64"
            FROM request_log
            WHERE ts >= datetime('now', ?1) AND ip IS NOT NULL
            GROUP BY substr(ts, 1, 10)
            ORDER BY substr(ts, 1, 10) ASC
            "#,
            w
        )
        .fetch_all(executor)
        .await?)
    }

    /// Top *content* paths over the window, excluding static assets + well-known
    /// files so real pages rank. The exclusion is a plain prefix/exact set —
    /// amend it here if a new static prefix shows up.
    pub async fn count_by_content_path(
        executor: impl SqliteExecutor<'_>,
        since_days: i64,
        limit: i64,
    ) -> Result<Vec<PathCount>> {
        let w = window(since_days);
        Ok(query_as!(
            PathCount,
            r#"
            SELECT path, COUNT(*) as "count!: i64"
            FROM request_log
            WHERE ts >= datetime('now', ?1)
              AND path NOT LIKE '/styles%'
              AND path NOT LIKE '/vendor%'
              AND path NOT LIKE '/scripts%'
              AND path NOT LIKE '/images%'
              AND path NOT LIKE '/attachments%'
              AND path NOT LIKE '/diagram%'
              AND path NOT IN ('/favicon.ico', '/manifest.webmanifest', '/robots.txt', '/apple-touch-icon.png')
            GROUP BY path
            ORDER BY COUNT(*) DESC, path ASC
            LIMIT ?2
            "#,
            w,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// Delete rows older than `retain_days`. Returns the number removed.
    pub async fn prune_before(executor: impl SqliteExecutor<'_>, retain_days: i64) -> Result<u64> {
        let w = window(retain_days);
        Ok(query!(
            r#"DELETE FROM request_log WHERE ts < datetime('now', ?1)"#,
            w
        )
        .execute(executor)
        .await?
        .rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    fn entry(path: &str, status: i64, ip: Option<&str>, ua: Option<&str>) -> NewRequestLog {
        NewRequestLog {
            method: "GET".to_string(),
            path: path.to_string(),
            status,
            ip: ip.map(String::from),
            user_agent: ua.map(String::from),
            referer: None,
        }
    }

    async fn seed(pool: &SqlitePool) -> Result<()> {
        for e in [
            entry("/pages/Resume", 200, Some("1.2.3.4"), Some("curl/8")),
            entry("/pages/Resume", 200, Some("1.2.3.4"), Some("curl/8")),
            entry("/pages/Resume", 200, Some("5.6.7.8"), Some("Mozilla/5")),
            entry("/login", 200, Some("5.6.7.8"), Some("Mozilla/5")),
            entry("/wp-admin", 404, None, None),
        ] {
            RequestLogDao::insert(pool, &e).await?;
        }
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn insert_and_recent(pool: SqlitePool) -> Result<()> {
        seed(&pool).await?;
        let recent = RequestLogDao::recent(&pool, 3).await?;
        assert_eq!(recent.len(), 3);
        // most recent first
        assert_eq!(recent[0].path, "/wp-admin");
        assert_eq!(recent[0].status, 404);
        assert!(recent[0].ip.is_none());
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn aggregates(pool: SqlitePool) -> Result<()> {
        seed(&pool).await?;

        assert_eq!(RequestLogDao::count_since(&pool, 1).await?, 5);
        assert_eq!(RequestLogDao::distinct_ip_count(&pool, 1).await?, 2);

        let by_path = RequestLogDao::count_by_content_path(&pool, 1, 10).await?;
        assert_eq!(by_path[0].path, "/pages/Resume");
        assert_eq!(by_path[0].count, 3);

        let by_ua = RequestLogDao::count_by_user_agent(&pool, 1, 10).await?;
        // curl/8 and Mozilla/5 each appear; curl/8 has 2, Mozilla/5 has 2, plus one NULL
        assert!(by_ua.iter().any(|u| u.user_agent.as_deref() == Some("curl/8") && u.count == 2));

        let by_day = RequestLogDao::count_by_day(&pool, 1).await?;
        assert_eq!(by_day.len(), 1); // all seeded "now"
        assert_eq!(by_day[0].count, 5);

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn prune(pool: SqlitePool) -> Result<()> {
        seed(&pool).await?;
        // an old row
        query!(
            "INSERT INTO request_log (ts, method, path, status) VALUES (datetime('now', '-100 days'), 'GET', '/old', 200)"
        )
        .execute(&pool)
        .await?;

        assert_eq!(RequestLogDao::count_since(&pool, 365).await?, 6);
        let removed = RequestLogDao::prune_before(&pool, 90).await?;
        assert_eq!(removed, 1);
        assert_eq!(RequestLogDao::count_since(&pool, 365).await?, 5);

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn content_path_excludes_static(pool: SqlitePool) -> Result<()> {
        for e in [
            entry("/pages/Resume", 200, Some("1.1.1.1"), None),
            entry("/pages/Resume", 200, Some("1.1.1.1"), None),
            entry("/blog/hello", 200, Some("2.2.2.2"), None),
            entry("/styles/main.css", 200, Some("1.1.1.1"), None),
            entry("/vendor/htmx/htmx.js", 200, Some("1.1.1.1"), None),
            entry("/diagram/abc123", 200, Some("1.1.1.1"), None),
            entry("/favicon.ico", 200, Some("1.1.1.1"), None),
        ] {
            RequestLogDao::insert(&pool, &e).await?;
        }

        let top = RequestLogDao::count_by_content_path(&pool, 1, 25).await?;
        let paths: Vec<&str> = top.iter().map(|p| p.path.as_str()).collect();

        assert!(paths.contains(&"/pages/Resume"), "content page must rank: {paths:?}");
        assert!(paths.contains(&"/blog/hello"));
        assert!(
            !paths.iter().any(|p| {
                p.starts_with("/styles")
                    || p.starts_with("/vendor")
                    || p.starts_with("/diagram")
                    || *p == "/favicon.ico"
            }),
            "static assets must be excluded: {paths:?}"
        );
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn unique_per_day_below_total(pool: SqlitePool) -> Result<()> {
        for e in [
            entry("/pages/Resume", 200, Some("1.1.1.1"), None),
            entry("/pages/Resume", 200, Some("1.1.1.1"), None), // same visitor
            entry("/pages/Resume", 200, Some("2.2.2.2"), None),
            entry("/pages/Resume", 200, None, None), // null ip — not a unique visitor
        ] {
            RequestLogDao::insert(&pool, &e).await?;
        }

        let total = RequestLogDao::count_by_day(&pool, 1).await?;
        let unique = RequestLogDao::distinct_ip_by_day(&pool, 1).await?;
        assert_eq!(total[0].count, 4, "4 total views");
        assert_eq!(unique[0].count, 2, "2 distinct IPs (null excluded)");
        Ok(())
    }
}
