//! Greylist + clearance persistence for the behavioral bot challenge (CX.1).
//!
//! Design + rationale: `docs/greylist-challenge-design.md`. Two tables:
//! - `greylist` — one row per abusive IP. Auto (behavioral) rows carry a SLIDING
//!   `expires_at`; a manual pin is `manual = true` + `expires_at = None` (never lapses).
//!   The request path reads an in-memory snapshot of the active set — the datetime()
//!   compares here run on the sweep timer, never per request.
//! - `greylist_clearance` — one row per solved toll (the "passing is a signal" data +
//!   the feed for the deferred clear-then-scan escalation).
//!
//! `expires_at` is stored as text and compared with `datetime(expires_at) > datetime('now')`
//! (NOT a raw string compare): the column can hold both the `CURRENT_TIMESTAMP` space-form
//! and the RFC3339 `T…+00:00` form sqlx writes, which sort differently as strings — same
//! gotcha as the scheduled-publishing gate (CU).

use anyhow::Result;
use sqlx::{
    prelude::FromRow,
    query, query_as,
    types::chrono::{DateTime, Utc},
    SqliteExecutor,
};

use crate::db::dao::request_log::Window;

/// An active or lapsed greylist row. `manual` distinguishes an admin pin (never lapses)
/// from a behavioral auto-entry (slides + expires).
#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct GreylistEntry {
    pub ip: String,
    /// Which rule tripped it (`"R1: signature probe"`, …) or `"manual"`.
    pub reason: String,
    /// Optional human/JSON evidence snapshot (the feature counts that tripped it).
    pub evidence: Option<String>,
    pub manual: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `None` for a manual pin (never lapses); a sliding cutoff for an auto entry.
    pub expires_at: Option<DateTime<Utc>>,
}

impl GreylistEntry {
    /// Whether this entry is enforceable at `now`: a manual pin always is; an auto entry
    /// is until its sliding `expires_at` passes. A non-manual row with no expiry is treated
    /// as inactive (shouldn't happen — auto upserts always set one — but fail open).
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.manual || self.expires_at.is_some_and(|e| e > now)
    }
}

/// A recorded toll solve. The clearance cookie is NOT IP-bound (see the design doc); this
/// records which IP solved for analytics + escalation only.
#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct Clearance {
    pub ip: String,
    pub cleared_at: DateTime<Utc>,
    /// Client-reported solve time in ms (signal — a suspiciously fast solve is a tell).
    pub solve_ms: Option<i64>,
    /// Which image version they solved against.
    pub digest_version: Option<i64>,
    pub user_agent: Option<String>,
}

/// A path that greylisted IPs probe and that NEVER succeeds for anyone — a candidate to add to
/// the R1 signature list (CX.9). Refinement is human-in-the-loop by design: this surfaces the
/// candidate, an admin adds worthy ones to `SIGNATURE_PATTERNS` + deploys (same retune-by-const
/// pattern as the `is_bot` markers).
#[derive(Clone, Debug, FromRow)]
pub struct CandidatePath {
    pub path: String,
    pub hits: i64,
    pub ips: i64,
}

pub struct GreylistDao;

impl GreylistDao {
    /// Upsert a behavioral (auto) greylist row. On an existing IP the expiry SLIDES to the
    /// new cutoff, `updated_at` bumps, and reason/evidence refresh to the latest trip. Does
    /// NOT touch `manual`, so re-tripping a manually-pinned IP keeps the pin (and the pin's
    /// `is_active` wins regardless of the expiry written here).
    pub async fn upsert_auto(
        executor: impl SqliteExecutor<'_>,
        ip: &str,
        reason: &str,
        evidence: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        query!(
            r#"
            INSERT INTO greylist (ip, reason, evidence, manual, expires_at)
            VALUES (?1, ?2, ?3, 0, ?4)
            ON CONFLICT(ip) DO UPDATE SET
                reason     = excluded.reason,
                evidence   = excluded.evidence,
                expires_at = excluded.expires_at,
                updated_at = CURRENT_TIMESTAMP
            "#,
            ip,
            reason,
            evidence,
            expires_at,
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Manually pin an IP: `manual = 1`, `expires_at = NULL` (never lapses until released).
    /// Upserts, so pinning an already-greylisted IP promotes it to a pin.
    pub async fn pin_manual(
        executor: impl SqliteExecutor<'_>,
        ip: &str,
        reason: &str,
    ) -> Result<()> {
        query!(
            r#"
            INSERT INTO greylist (ip, reason, evidence, manual, expires_at)
            VALUES (?1, ?2, NULL, 1, NULL)
            ON CONFLICT(ip) DO UPDATE SET
                reason     = excluded.reason,
                manual     = 1,
                expires_at = NULL,
                updated_at = CURRENT_TIMESTAMP
            "#,
            ip,
            reason,
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Remove an IP from the greylist (admin release or manual un-pin). Returns rows removed.
    pub async fn release(executor: impl SqliteExecutor<'_>, ip: &str) -> Result<u64> {
        Ok(query!("DELETE FROM greylist WHERE ip = ?1", ip)
            .execute(executor)
            .await?
            .rows_affected())
    }

    /// All currently-ACTIVE entries (manual pins + not-yet-expired auto rows), newest-touched
    /// first. Feeds the admin panel and the sweep's refresh of the request-path in-memory set.
    pub async fn active(executor: impl SqliteExecutor<'_>) -> Result<Vec<GreylistEntry>> {
        Ok(query_as!(
            GreylistEntry,
            r#"
            SELECT
                ip,
                reason,
                evidence,
                manual as "manual!: bool",
                created_at as "created_at!: DateTime<Utc>",
                updated_at as "updated_at!: DateTime<Utc>",
                expires_at as "expires_at?: DateTime<Utc>"
            FROM greylist
            WHERE manual = 1
               OR (expires_at IS NOT NULL AND datetime(expires_at) > datetime('now'))
            ORDER BY updated_at DESC
            "#
        )
        .fetch_all(executor)
        .await?)
    }

    /// Record a solved toll. `digest_version`/`solve_ms`/`user_agent` are best-effort signal.
    pub async fn record_clearance(
        executor: impl SqliteExecutor<'_>,
        ip: &str,
        solve_ms: Option<i64>,
        digest_version: Option<i64>,
        user_agent: Option<&str>,
    ) -> Result<()> {
        query!(
            r#"
            INSERT INTO greylist_clearance (ip, solve_ms, digest_version, user_agent)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            ip,
            solve_ms,
            digest_version,
            user_agent,
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Most-recent clearances for the admin panel, newest first.
    pub async fn recent_clearances(
        executor: impl SqliteExecutor<'_>,
        limit: i64,
    ) -> Result<Vec<Clearance>> {
        Ok(query_as!(
            Clearance,
            r#"
            SELECT
                ip,
                cleared_at as "cleared_at!: DateTime<Utc>",
                solve_ms,
                digest_version,
                user_agent
            FROM greylist_clearance
            ORDER BY id DESC
            LIMIT ?1
            "#,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// Housekeeping: drop lapsed auto entries (manual pins are kept). `active()` already
    /// filters these out of reads — this just keeps the table small. Returns rows removed.
    pub async fn prune_expired(executor: impl SqliteExecutor<'_>) -> Result<u64> {
        Ok(query!(
            r#"
            DELETE FROM greylist
            WHERE manual = 0
              AND expires_at IS NOT NULL
              AND datetime(expires_at) <= datetime('now')
            "#
        )
        .execute(executor)
        .await?
        .rows_affected())
    }

    /// Candidate R1 signatures (CX.9): paths that a currently-greylisted IP probed AND that NEVER
    /// returned success for ANYONE (so adding one to R1 can't false-positive a real visitor).
    /// Ordered by hit volume. The caller filters out paths the current ruleset already matches.
    pub async fn candidate_signatures(
        executor: impl SqliteExecutor<'_>,
        limit: i64,
    ) -> Result<Vec<CandidatePath>> {
        Ok(query_as!(
            CandidatePath,
            r#"
            SELECT path, COUNT(*) as "hits!: i64", COUNT(DISTINCT ip) as "ips!: i64"
            FROM request_log
            WHERE path IN (
                SELECT DISTINCT path FROM request_log WHERE ip IN (SELECT ip FROM greylist)
            )
            GROUP BY path
            HAVING SUM(CASE WHEN status < 400 THEN 1 ELSE 0 END) = 0
            ORDER BY COUNT(*) DESC
            LIMIT ?1
            "#,
            limit
        )
        .fetch_all(executor)
        .await?)
    }

    /// Toll SOLVES over the window (CY.8) — the "got through" numerator. `cleared_at` can hold
    /// either the space or the RFC3339 datetime form, so normalize with `datetime()`.
    pub async fn count_clearances_since(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
    ) -> Result<i64> {
        Ok(query!(
            r#"
            SELECT COUNT(*) as "count!: i64"
            FROM greylist_clearance
            WHERE datetime(cleared_at) >= datetime(?1) AND datetime(cleared_at) < datetime(?2)
            "#,
            w.from,
            w.to
        )
        .fetch_one(executor)
        .await?
        .count)
    }

    /// Distinct IPs that solved the toll over the window (CY.8) — the numerator for the IP-based
    /// solve rate (paired with `RequestLogDao::distinct_challenged_ips`).
    pub async fn distinct_cleared_ips_since(
        executor: impl SqliteExecutor<'_>,
        w: &Window,
    ) -> Result<i64> {
        Ok(query!(
            r#"
            SELECT COUNT(DISTINCT ip) as "count!: i64"
            FROM greylist_clearance
            WHERE datetime(cleared_at) >= datetime(?1) AND datetime(cleared_at) < datetime(?2)
            "#,
            w.from,
            w.to
        )
        .fetch_one(executor)
        .await?
        .count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    fn hours_from_now(h: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(Utc::now().timestamp() + h * 3600, 0).unwrap()
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn upsert_auto_is_idempotent_per_ip_and_slides_expiry(pool: SqlitePool) -> Result<()> {
        GreylistDao::upsert_auto(&pool, "1.2.3.4", "R2: 404 burst", Some("distinct_404=9"), hours_from_now(1))
            .await?;
        // Re-trip: same IP, later expiry, new reason/evidence.
        GreylistDao::upsert_auto(&pool, "1.2.3.4", "R1: signature probe", Some("php=3"), hours_from_now(48))
            .await?;

        let active = GreylistDao::active(&pool).await?;
        assert_eq!(active.len(), 1, "one row per IP");
        let e = &active[0];
        assert_eq!(e.reason, "R1: signature probe", "latest trip wins");
        assert_eq!(e.evidence.as_deref(), Some("php=3"));
        assert!(!e.manual);
        assert!(e.expires_at.unwrap() > hours_from_now(40), "expiry slid forward");
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn expired_auto_rows_are_inactive_manual_pins_never_lapse(pool: SqlitePool) -> Result<()> {
        GreylistDao::upsert_auto(&pool, "9.9.9.9", "R3: flood", None, hours_from_now(-1)).await?; // already lapsed
        GreylistDao::upsert_auto(&pool, "8.8.8.8", "R2: 404 burst", None, hours_from_now(24)).await?; // fresh
        GreylistDao::pin_manual(&pool, "7.7.7.7", "manual").await?;

        let active = GreylistDao::active(&pool).await?;
        let ips: Vec<&str> = active.iter().map(|e| e.ip.as_str()).collect();
        assert!(!ips.contains(&"9.9.9.9"), "lapsed auto row is inactive");
        assert!(ips.contains(&"8.8.8.8"), "fresh auto row is active");
        assert!(ips.contains(&"7.7.7.7"), "manual pin is active");

        let pin = active.iter().find(|e| e.ip == "7.7.7.7").unwrap();
        assert!(pin.manual);
        assert!(pin.expires_at.is_none(), "a manual pin has no expiry");
        assert!(pin.is_active(hours_from_now(24 * 3650)), "manual pin active far in the future");
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn pin_then_auto_retrip_keeps_the_pin(pool: SqlitePool) -> Result<()> {
        GreylistDao::pin_manual(&pool, "5.5.5.5", "manual").await?;
        // An auto sweep later re-trips the same IP with a (short, even lapsed) expiry.
        GreylistDao::upsert_auto(&pool, "5.5.5.5", "R2: 404 burst", None, hours_from_now(-1)).await?;

        let active = GreylistDao::active(&pool).await?;
        let e = active.iter().find(|e| e.ip == "5.5.5.5").expect("pin survives the re-trip");
        assert!(e.manual, "manual flag is not cleared by an auto upsert");
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn release_removes_the_row(pool: SqlitePool) -> Result<()> {
        GreylistDao::upsert_auto(&pool, "1.1.1.1", "R3: flood", None, hours_from_now(24)).await?;
        assert_eq!(GreylistDao::release(&pool, "1.1.1.1").await?, 1);
        assert_eq!(GreylistDao::release(&pool, "1.1.1.1").await?, 0, "already gone");
        assert!(GreylistDao::active(&pool).await?.is_empty());
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn prune_drops_lapsed_auto_keeps_manual_and_fresh(pool: SqlitePool) -> Result<()> {
        GreylistDao::upsert_auto(&pool, "9.9.9.9", "R3: flood", None, hours_from_now(-1)).await?;
        GreylistDao::upsert_auto(&pool, "8.8.8.8", "R2: 404 burst", None, hours_from_now(24)).await?;
        GreylistDao::pin_manual(&pool, "7.7.7.7", "manual").await?;

        assert_eq!(GreylistDao::prune_expired(&pool).await?, 1, "only the lapsed auto row");
        let active = GreylistDao::active(&pool).await?;
        assert_eq!(active.len(), 2);
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn clearances_roundtrip_newest_first(pool: SqlitePool) -> Result<()> {
        GreylistDao::record_clearance(&pool, "2.2.2.2", Some(850), Some(1), Some("Mozilla/5.0")).await?;
        GreylistDao::record_clearance(&pool, "3.3.3.3", None, Some(1), None).await?;

        let recent = GreylistDao::recent_clearances(&pool, 10).await?;
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].ip, "3.3.3.3", "newest first");
        assert_eq!(recent[1].ip, "2.2.2.2");
        assert_eq!(recent[1].solve_ms, Some(850));
        assert_eq!(recent[1].user_agent.as_deref(), Some("Mozilla/5.0"));
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn clearance_counts_over_the_window(pool: SqlitePool) -> Result<()> {
        GreylistDao::record_clearance(&pool, "1.1.1.1", Some(500), None, None).await?;
        GreylistDao::record_clearance(&pool, "1.1.1.1", None, None, None).await?; // same IP, twice
        GreylistDao::record_clearance(&pool, "2.2.2.2", None, None, None).await?;

        let all = Window::custom(None, None);
        assert_eq!(
            GreylistDao::count_clearances_since(&pool, &all).await?,
            3,
            "all solves counted (the solve-rate numerator, request-based)"
        );
        assert_eq!(
            GreylistDao::distinct_cleared_ips_since(&pool, &all).await?,
            2,
            "distinct solving IPs (the IP-based solve rate)"
        );
        Ok(())
    }
}
