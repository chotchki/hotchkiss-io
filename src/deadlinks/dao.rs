//! DL.5 — persistence + confirm-before-alarm.
//!
//! `link_check` is one row per DISTINCT url with a `consecutive_failures` streak;
//! `link_ref` is the page↔url mapping refreshed each scan. The streak math
//! (`next_state`) is a PURE function — the confirm-before-alarm heart — tested
//! without a DB.

use anyhow::Result;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{query, query_as, SqliteExecutor, SqlitePool};

use super::class::{CheckClass, LinkKind};

/// A link is "confirmed dead" only after this many consecutive daily failures whose
/// LATEST verdict is `dead` — three bad passes is a signal, one is noise.
pub const CONFIRM_THRESHOLD: i64 = 3;

/// The streak advances at most once per this interval (~a day). A manual re-check
/// within it updates status but doesn't inflate the count, so "N failures" means
/// "N daily passes", not "clicked re-check N times".
pub const MIN_STREAK_INTERVAL_SECS: i64 = 20 * 3600;

/// Drop `link_check` rows no page references any more once they're older than this.
pub const RETAIN_DAYS: i64 = 30;

/// One `link_check` row — a url's current verdict + its failure streak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCheckRow {
    pub url: String,
    pub kind: String,
    pub last_class: String,
    pub last_status: Option<i64>,
    pub detail: Option<String>,
    pub consecutive_failures: i64,
    pub first_failed_at: Option<DateTime<Utc>>,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_checked_at: DateTime<Utc>,
}

impl LinkCheckRow {
    pub fn class(&self) -> CheckClass {
        CheckClass::from_stored(&self.last_class)
    }

    pub fn kind(&self) -> LinkKind {
        LinkKind::from_stored(&self.kind)
    }

    /// Dead for `CONFIRM_THRESHOLD` consecutive daily passes AND the latest verdict
    /// is a hard `dead` (a `transient` flap accrues the streak but never confirms).
    pub fn is_confirmed_dead(&self) -> bool {
        self.class() == CheckClass::Dead && self.consecutive_failures >= CONFIRM_THRESHOLD
    }

    /// Failing (dead or transient) but not YET confirmed — the early-warning bucket.
    pub fn is_failing(&self) -> bool {
        self.class().counts_as_failure() && !self.is_confirmed_dead()
    }

    /// Bot-walled (`blocked`) or an unrecognized internal route (`unknown`) — the
    /// "verify by hand" bucket, NOT counted as broken.
    pub fn needs_review(&self) -> bool {
        matches!(self.class(), CheckClass::Blocked | CheckClass::Unknown)
    }
}

/// Compute the next `link_check` row from the previous one + a fresh verdict. PURE
/// (no DB, no clock) so the streak logic is exhaustively testable.
///
/// - `Ok` → streak 0, stamp `last_ok_at`, clear `first_failed_at`.
/// - `Dead`/`Transient` → advance the streak (at most once per `min_streak_interval`),
///   keep/stamp `first_failed_at`.
/// - `Blocked`/`Unknown` → hold the streak + failure timestamps (orthogonal: we
///   couldn't determine liveness, so neither advance nor reset).
#[allow(clippy::too_many_arguments)]
pub fn next_state(
    url: &str,
    kind: LinkKind,
    prev: Option<&LinkCheckRow>,
    class: CheckClass,
    status: Option<u16>,
    detail: &str,
    now: DateTime<Utc>,
    min_streak_interval_secs: i64,
) -> LinkCheckRow {
    let prev_failures = prev.map(|p| p.consecutive_failures).unwrap_or(0);
    let prev_first_failed = prev.and_then(|p| p.first_failed_at);
    let prev_last_ok = prev.and_then(|p| p.last_ok_at);

    let (consecutive_failures, first_failed_at, last_ok_at) = if class == CheckClass::Ok {
        (0, None, Some(now))
    } else if class.counts_as_failure() {
        let within_interval = prev
            .map(|p| now.timestamp() - p.last_checked_at.timestamp() < min_streak_interval_secs)
            .unwrap_or(false);
        // A failing link is always at least 1 in the streak (even if the previous
        // state was a non-counting Blocked/Unknown at 0).
        let advanced = if within_interval {
            prev_failures.max(1)
        } else {
            prev_failures + 1
        };
        (advanced, prev_first_failed.or(Some(now)), prev_last_ok)
    } else {
        (prev_failures, prev_first_failed, prev_last_ok)
    };

    LinkCheckRow {
        url: url.to_string(),
        kind: kind.as_str().to_string(),
        last_class: class.as_str().to_string(),
        last_status: status.map(|s| s as i64),
        detail: (!detail.trim().is_empty()).then(|| detail.trim().to_string()),
        consecutive_failures,
        first_failed_at,
        last_ok_at,
        last_checked_at: now,
    }
}

pub struct LinkCheckDao;

impl LinkCheckDao {
    pub async fn get(executor: impl SqliteExecutor<'_>, url: &str) -> Result<Option<LinkCheckRow>> {
        let row = query_as!(
            LinkCheckRow,
            r#"
            SELECT url, kind, last_class,
                   last_status as "last_status: i64",
                   detail,
                   consecutive_failures as "consecutive_failures!: i64",
                   first_failed_at as "first_failed_at: DateTime<Utc>",
                   last_ok_at as "last_ok_at: DateTime<Utc>",
                   last_checked_at as "last_checked_at!: DateTime<Utc>"
            FROM link_check WHERE url = ?1
            "#,
            url
        )
        .fetch_optional(executor)
        .await?;
        Ok(row)
    }

    /// Upsert a computed row (one url).
    pub async fn upsert(executor: impl SqliteExecutor<'_>, row: &LinkCheckRow) -> Result<()> {
        query!(
            r#"
            INSERT INTO link_check
                (url, kind, last_class, last_status, detail,
                 consecutive_failures, first_failed_at, last_ok_at, last_checked_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(url) DO UPDATE SET
                kind = ?2, last_class = ?3, last_status = ?4, detail = ?5,
                consecutive_failures = ?6, first_failed_at = ?7,
                last_ok_at = ?8, last_checked_at = ?9
            "#,
            row.url,
            row.kind,
            row.last_class,
            row.last_status,
            row.detail,
            row.consecutive_failures,
            row.first_failed_at,
            row.last_ok_at,
            row.last_checked_at,
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Read the previous state, fold in the fresh verdict via `next_state`, persist,
    /// and return the new row. This is the one call the scan + the per-URL re-check
    /// both go through.
    pub async fn record(
        pool: &SqlitePool,
        url: &str,
        kind: LinkKind,
        class: CheckClass,
        status: Option<u16>,
        detail: &str,
        now: DateTime<Utc>,
    ) -> Result<LinkCheckRow> {
        let prev = Self::get(pool, url).await?;
        let next = next_state(
            url,
            kind,
            prev.as_ref(),
            class,
            status,
            detail,
            now,
            MIN_STREAK_INTERVAL_SECS,
        );
        Self::upsert(pool, &next).await?;
        Ok(next)
    }

    /// Every non-ok row, worst-streak first — the admin view buckets these in Rust
    /// (confirmed-dead / failing / needs-review).
    pub async fn problem_rows(executor: impl SqliteExecutor<'_>) -> Result<Vec<LinkCheckRow>> {
        let rows = query_as!(
            LinkCheckRow,
            r#"
            SELECT url, kind, last_class,
                   last_status as "last_status: i64",
                   detail,
                   consecutive_failures as "consecutive_failures!: i64",
                   first_failed_at as "first_failed_at: DateTime<Utc>",
                   last_ok_at as "last_ok_at: DateTime<Utc>",
                   last_checked_at as "last_checked_at!: DateTime<Utc>"
            FROM link_check
            WHERE last_class != 'ok'
            ORDER BY consecutive_failures DESC, url ASC
            "#
        )
        .fetch_all(executor)
        .await?;
        Ok(rows)
    }

    /// Total distinct urls tracked + how many are currently ok (for the header).
    pub async fn counts(executor: impl SqliteExecutor<'_>) -> Result<(i64, i64)> {
        let row = query!(
            r#"SELECT COUNT(*) as "total!: i64",
                      COALESCE(SUM(CASE WHEN last_class = 'ok' THEN 1 ELSE 0 END), 0) as "ok!: i64"
               FROM link_check"#
        )
        .fetch_one(executor)
        .await?;
        Ok((row.total, row.ok))
    }

    /// The most recent check time across all urls — the "last scanned" header value.
    pub async fn last_checked(executor: impl SqliteExecutor<'_>) -> Result<Option<DateTime<Utc>>> {
        let row = query!(
            r#"SELECT MAX(last_checked_at) as "ts: DateTime<Utc>" FROM link_check"#
        )
        .fetch_one(executor)
        .await?;
        Ok(row.ts)
    }

    /// Drop rows no page references any more once they're older than `retain_days`.
    pub async fn prune_orphans(executor: impl SqliteExecutor<'_>, retain_days: i64) -> Result<u64> {
        let modifier = format!("-{retain_days} days");
        Ok(query!(
            r#"
            DELETE FROM link_check
            WHERE url NOT IN (SELECT url FROM link_ref)
              AND datetime(last_checked_at) < datetime('now', ?1)
            "#,
            modifier
        )
        .execute(executor)
        .await?
        .rows_affected())
    }
}

pub struct LinkRefDao;

impl LinkRefDao {
    /// Replace a page's refs with the current set (delete-then-insert in a tx) so the
    /// mapping always reflects live content.
    pub async fn replace_for_page(pool: &SqlitePool, page_id: i64, urls: &[String]) -> Result<()> {
        let mut tx = pool.begin().await?;
        query!("DELETE FROM link_ref WHERE page_id = ?1", page_id)
            .execute(&mut *tx)
            .await?;
        for url in urls {
            query!(
                "INSERT OR IGNORE INTO link_ref (page_id, url) VALUES (?1, ?2)",
                page_id,
                url
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The page ids that reference `url` (for the "grouped by page" admin view).
    pub async fn pages_for_url(executor: impl SqliteExecutor<'_>, url: &str) -> Result<Vec<i64>> {
        let rows = query!("SELECT page_id FROM link_ref WHERE url = ?1", url)
            .fetch_all(executor)
            .await?;
        Ok(rows.into_iter().map(|r| r.page_id).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::types::chrono::TimeZone;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }
    const DAY: i64 = 24 * 3600;
    const INTERVAL: i64 = MIN_STREAK_INTERVAL_SECS;

    #[test]
    fn ok_resets_the_streak() {
        let dead = next_state("u", LinkKind::External, None, CheckClass::Dead, Some(404), "gone", at(0), INTERVAL);
        assert_eq!(dead.consecutive_failures, 1);
        let ok = next_state("u", LinkKind::External, Some(&dead), CheckClass::Ok, Some(200), "HTTP 200", at(DAY), INTERVAL);
        assert_eq!(ok.consecutive_failures, 0);
        assert!(ok.first_failed_at.is_none());
        assert_eq!(ok.last_ok_at, Some(at(DAY)));
    }

    #[test]
    fn three_daily_dead_passes_confirm() {
        // Daily passes accrue a streak; at 3 with last_class=dead it's confirmed.
        let mut row = next_state("u", LinkKind::External, None, CheckClass::Dead, Some(404), "", at(0), INTERVAL);
        assert!(!row.is_confirmed_dead());
        row = next_state("u", LinkKind::External, Some(&row), CheckClass::Dead, Some(404), "", at(DAY), INTERVAL);
        assert!(!row.is_confirmed_dead());
        row = next_state("u", LinkKind::External, Some(&row), CheckClass::Dead, Some(404), "", at(2 * DAY), INTERVAL);
        assert_eq!(row.consecutive_failures, 3);
        assert!(row.is_confirmed_dead(), "3 daily dead passes = confirmed");
        // first_failed_at stays pinned to the START of the streak.
        assert_eq!(row.first_failed_at, Some(at(0)));
    }

    #[test]
    fn manual_recheck_within_a_day_does_not_inflate_the_streak() {
        // Two checks 1h apart must NOT count as 2 daily failures.
        let a = next_state("u", LinkKind::External, None, CheckClass::Dead, Some(404), "", at(0), INTERVAL);
        assert_eq!(a.consecutive_failures, 1);
        let b = next_state("u", LinkKind::External, Some(&a), CheckClass::Dead, Some(404), "", at(3600), INTERVAL);
        assert_eq!(b.consecutive_failures, 1, "same-day re-check holds the streak");
    }

    #[test]
    fn transient_accrues_streak_but_never_confirms() {
        // A site 5xx-flapping for days accrues the streak but stays labeled transient
        // — never confirmed-dead (which requires last_class = dead).
        let mut row = next_state("u", LinkKind::External, None, CheckClass::Transient, Some(503), "", at(0), INTERVAL);
        for d in 1..6 {
            row = next_state("u", LinkKind::External, Some(&row), CheckClass::Transient, Some(503), "", at(d * DAY), INTERVAL);
        }
        assert!(row.consecutive_failures >= CONFIRM_THRESHOLD);
        assert!(!row.is_confirmed_dead(), "transient never confirms as dead");
        assert!(row.is_failing(), "but it IS surfaced as failing");
    }

    #[test]
    fn blocked_holds_the_streak_and_is_review_not_dead() {
        let dead = next_state("u", LinkKind::External, None, CheckClass::Dead, Some(404), "", at(0), INTERVAL);
        let blocked = next_state("u", LinkKind::External, Some(&dead), CheckClass::Blocked, Some(403), "", at(DAY), INTERVAL);
        assert_eq!(blocked.consecutive_failures, 1, "blocked neither advances nor resets");
        assert!(blocked.needs_review());
        assert!(!blocked.is_confirmed_dead());
        assert!(!blocked.is_failing());
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn record_roundtrips_and_confirms_over_days(pool: SqlitePool) {
        for d in 0..3 {
            LinkCheckDao::record(&pool, "https://x.example/gone", LinkKind::External, CheckClass::Dead, Some(404), "HTTP 404", at(d * DAY))
                .await
                .unwrap();
        }
        let row = LinkCheckDao::get(&pool, "https://x.example/gone").await.unwrap().unwrap();
        assert_eq!(row.consecutive_failures, 3);
        assert!(row.is_confirmed_dead());

        let problems = LinkCheckDao::problem_rows(&pool).await.unwrap();
        assert_eq!(problems.len(), 1);

        // A fix (Ok) drops it out of the problem set.
        LinkCheckDao::record(&pool, "https://x.example/gone", LinkKind::External, CheckClass::Ok, Some(200), "HTTP 200", at(3 * DAY))
            .await
            .unwrap();
        assert!(LinkCheckDao::problem_rows(&pool).await.unwrap().is_empty());
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn refs_replace_and_orphans_prune(pool: SqlitePool) {
        use crate::db::dao::content_pages::ContentPageDao;
        let page = ContentPageDao::create(&pool, None, "p".to_string(), None, "x".to_string(), None)
            .await
            .unwrap();

        LinkRefDao::replace_for_page(&pool, page.page_id, &["https://a.example/".to_string(), "https://b.example/".to_string()])
            .await
            .unwrap();
        assert_eq!(LinkRefDao::pages_for_url(&pool, "https://a.example/").await.unwrap(), vec![page.page_id]);

        // Re-scan drops b, keeps a.
        LinkRefDao::replace_for_page(&pool, page.page_id, &["https://a.example/".to_string()])
            .await
            .unwrap();
        assert!(LinkRefDao::pages_for_url(&pool, "https://b.example/").await.unwrap().is_empty());

        // An orphaned, old link_check row prunes; a referenced one stays.
        LinkCheckDao::record(&pool, "https://a.example/", LinkKind::External, CheckClass::Ok, Some(200), "", at(0)).await.unwrap();
        LinkCheckDao::upsert(&pool, &LinkCheckRow {
            url: "https://orphan.example/".to_string(),
            kind: "external".to_string(),
            last_class: "dead".to_string(),
            last_status: Some(404),
            detail: None,
            consecutive_failures: 5,
            first_failed_at: None,
            last_ok_at: None,
            last_checked_at: at(0), // ancient
        }).await.unwrap();
        let pruned = LinkCheckDao::prune_orphans(&pool, RETAIN_DAYS).await.unwrap();
        assert_eq!(pruned, 1, "only the un-referenced ancient row prunes");
        assert!(LinkCheckDao::get(&pool, "https://a.example/").await.unwrap().is_some());
        assert!(LinkCheckDao::get(&pool, "https://orphan.example/").await.unwrap().is_none());
    }
}
