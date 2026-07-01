//! One-shot startup backfill (Phase CR.2): stamp the stored `is_bot` column for rows
//! logged BEFORE it existed (they carry `is_bot NULL`). Runs DETACHED after boot — never
//! in the coordinator's `try_join!`, so a failure can't take the app down and it doesn't
//! delay serving. Idempotent: it only touches `is_bot IS NULL` rows (`reclassify_bots(_,
//! true)`), so a restart mid-run resumes cleanly and a steady-state boot is a single cheap
//! `SELECT DISTINCT … WHERE is_bot IS NULL`. A legacy row classifies as "neither" until
//! stamped — a transient audience undercount only during this run.

use sqlx::SqlitePool;

use crate::db::dao::request_log::RequestLogDao;

/// Spawn the backfill as a detached background task. It logs its own outcome and never
/// bubbles — the caller (the coordinator) does not await it.
pub fn spawn(pool: SqlitePool) {
    tokio::spawn(async move {
        match RequestLogDao::reclassify_bots(&pool, true).await {
            Ok(0) => {} // steady state — nothing to stamp
            Ok(n) => tracing::info!("is_bot backfill: stamped {n} legacy request_log rows"),
            Err(e) => tracing::error!("is_bot backfill aborted: {e:?}"),
        }
    });
}
