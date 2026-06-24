//! Daily on-disk SQLite backups.
//!
//! Snapshots the live database with `VACUUM INTO`, which writes a consistent
//! point-in-time copy without blocking writers (no external `sqlite3` binary —
//! everything runs in-process through the existing sqlx pool). Backups land in a
//! dated file and a rolling window of the most recent few is kept; the whole
//! server is already covered off-site by Backblaze, so we only need the files to
//! exist on disk.

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use sqlx::types::chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

/// How many of the most recent dated backups to keep on disk.
pub const RETAIN_BACKUPS: usize = 7;

/// Prefix + suffix that bracket the `YYYY-MM-DD` date in a backup filename.
const BACKUP_PREFIX: &str = "database-";
const BACKUP_SUFFIX: &str = ".sqlite";

/// Write a dated snapshot of `pool`'s database into `dir` and return its path.
///
/// The destination is `database-YYYY-MM-DD.sqlite` (UTC date). `VACUUM INTO`
/// refuses to overwrite, so if today's file already exists it is removed first
/// (a same-day re-run simply refreshes the snapshot). The directory is created
/// if it does not exist.
pub async fn run_backup(pool: &SqlitePool, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("creating backup directory {}", dir.display()))?;

    let date = Utc::now().format("%Y-%m-%d");
    let dest = dir.join(format!("{BACKUP_PREFIX}{date}{BACKUP_SUFFIX}"));

    // VACUUM INTO errors if the destination already exists, so clear a same-day
    // snapshot before re-taking it.
    if dest.exists() {
        fs::remove_file(&dest)
            .with_context(|| format!("removing stale backup {}", dest.display()))?;
    }

    // The destination path is interpolated into SQL rather than bound because
    // VACUUM INTO does not accept a bound parameter for its target. The path is
    // operator-controlled config, not user input; we still escape single quotes
    // defensively to keep the string literal well-formed.
    let dest_str = dest
        .to_str()
        .with_context(|| format!("backup path is not valid UTF-8: {}", dest.display()))?;
    let escaped = dest_str.replace('\'', "''");
    let sql = format!("VACUUM INTO '{escaped}'");

    sqlx::query(&sql)
        .execute(pool)
        .await
        .with_context(|| format!("VACUUM INTO {}", dest.display()))?;

    Ok(dest)
}

/// Delete dated backups in `dir` beyond the newest `keep`.
///
/// Files are ordered by their `YYYY-MM-DD` filename (lexicographic == temporal
/// for this format); only files matching the backup naming scheme are
/// considered, so unrelated files in the directory are left alone.
pub fn prune_old_backups(dir: &Path, keep: usize) -> Result<()> {
    let mut backups: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading backup directory {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| is_backup_file(path))
        .collect();

    // Newest last (filename date sorts chronologically).
    backups.sort();

    if backups.len() <= keep {
        return Ok(());
    }

    let remove_count = backups.len() - keep;
    for path in backups.into_iter().take(remove_count) {
        fs::remove_file(&path)
            .with_context(|| format!("removing old backup {}", path.display()))?;
    }

    Ok(())
}

/// True for files named `database-<something>.sqlite`.
fn is_backup_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => {
            name.starts_with(BACKUP_PREFIX)
                && name.ends_with(BACKUP_SUFFIX)
                && name.len() > BACKUP_PREFIX.len() + BACKUP_SUFFIX.len()
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::query;
    use std::fs::File;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn backup_produces_readable_copy(pool: SqlitePool) -> Result<()> {
        // Seed a known row so we can confirm it survives into the snapshot.
        query!(
            "INSERT INTO request_log (method, path, status) VALUES ('GET', '/backup-marker', 200)"
        )
        .execute(&pool)
        .await?;

        let dir = tempfile::tempdir()?;
        let dest = run_backup(&pool, dir.path()).await?;
        assert!(dest.exists(), "backup file should exist");

        // Open the snapshot as a fresh, independent database and confirm the row.
        let backup_url = format!("sqlite://{}", dest.display());
        let backup_pool = SqlitePool::connect(&backup_url).await?;
        let row = query!("SELECT path FROM request_log WHERE path = '/backup-marker'")
            .fetch_optional(&backup_pool)
            .await?;
        backup_pool.close().await;

        assert!(row.is_some(), "seeded row should be present in the backup");
        assert_eq!(row.unwrap().path, "/backup-marker");

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn backup_refreshes_same_day_snapshot(pool: SqlitePool) -> Result<()> {
        let dir = tempfile::tempdir()?;
        // Two runs on the same UTC day must not error on the "destination
        // exists" VACUUM INTO restriction, and must reuse the same filename.
        let first = run_backup(&pool, dir.path()).await?;
        let second = run_backup(&pool, dir.path()).await?;
        assert_eq!(first, second);
        assert!(second.exists());
        Ok(())
    }

    #[test]
    fn prune_keeps_exactly_seven_newest() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // 10 dated backups, oldest..newest.
        let dates = [
            "2026-01-01",
            "2026-01-02",
            "2026-01-03",
            "2026-01-04",
            "2026-01-05",
            "2026-01-06",
            "2026-01-07",
            "2026-01-08",
            "2026-01-09",
            "2026-01-10",
        ];
        for d in dates {
            File::create(dir.path().join(format!("database-{d}.sqlite")))?;
        }
        // An unrelated file that must be left untouched.
        File::create(dir.path().join("notes.txt"))?;

        prune_old_backups(dir.path(), RETAIN_BACKUPS)?;

        let mut remaining: Vec<String> = fs::read_dir(dir.path())?
            .filter_map(|e| e.ok())
            .filter(|e| is_backup_file(&e.path()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();

        assert_eq!(remaining.len(), RETAIN_BACKUPS, "should keep exactly 7");
        assert_eq!(remaining.first().unwrap(), "database-2026-01-04.sqlite");
        assert_eq!(remaining.last().unwrap(), "database-2026-01-10.sqlite");

        // The 8th-oldest (2026-01-03) and everything before it is gone.
        assert!(!dir.path().join("database-2026-01-03.sqlite").exists());
        assert!(!dir.path().join("database-2026-01-01.sqlite").exists());
        // Non-backup file untouched.
        assert!(dir.path().join("notes.txt").exists());

        Ok(())
    }

    #[test]
    fn prune_noop_when_under_limit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        for d in ["2026-02-01", "2026-02-02", "2026-02-03"] {
            File::create(dir.path().join(format!("database-{d}.sqlite")))?;
        }
        prune_old_backups(dir.path(), RETAIN_BACKUPS)?;
        let count = fs::read_dir(dir.path())?
            .filter_map(|e| e.ok())
            .filter(|e| is_backup_file(&e.path()))
            .count();
        assert_eq!(count, 3);
        Ok(())
    }
}
