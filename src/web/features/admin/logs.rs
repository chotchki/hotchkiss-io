//! `/admin/logs` — a bounded, newest-first tail of the app log (Phase CO), so an
//! admin can see what a deploy / background task actually did WITHOUT ssh + a
//! manual file grab (the v0.0.81 backfill silent-no-op + the CN.12 wedged-root
//! timeout both motivated this — those `tracing::error!`s now surface in one
//! click).
//!
//! NO INFINITE LOOP: the page has a MANUAL refresh link (no auto-poll), AND the
//! route is excluded from the `request_log` middleware (see request_log.rs), so
//! viewing the log never feeds the access log it's tailing.

use askama::Template;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::web::{
    app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
    features::top_bar::TopBar, html_template::HtmlTemplate, session::SessionData,
};

/// Read at most the last 256 KiB of EACH log file — NEVER slurp a multi-GB file.
const TAIL_BYTES: u64 = 256 * 1024;
/// And show at most this many lines (after the level filter), newest-first.
const TAIL_LINES: usize = 800;
/// Walk back at most this many daily-rotated files looking for `TAIL_LINES`
/// matches (DM.9). Bounds the work while covering ~a work-week — enough that an
/// ERROR which rotated into an older file is still found, not structurally
/// hidden the moment today's file fills with newer noise.
const MAX_FILES: usize = 5;

/// Level filter for the viewer. `Warn` means warn-AND-above (warn + error).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    All,
    Warn,
    Error,
}

impl LogLevel {
    fn parse(s: Option<&str>) -> LogLevel {
        match s {
            Some("error") => LogLevel::Error,
            Some("warn") => LogLevel::Warn,
            _ => LogLevel::All,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::All => "all",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
    /// Does a compact-format line (`<ts>  LEVEL target: msg`) pass this filter? The
    /// level is the 2nd whitespace token; a line with no parseable level (a
    /// continuation line) only shows under `All`.
    fn matches(self, line: &str) -> bool {
        if self == LogLevel::All {
            return true;
        }
        match line.split_whitespace().nth(1) {
            Some("ERROR") => true,
            Some("WARN") => self == LogLevel::Warn,
            _ => false,
        }
    }
}

#[derive(Deserialize)]
pub struct LogsQuery {
    pub level: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/logs.html")]
pub struct LogsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    /// "all" | "warn" | "error" — drives which filter chip is active + the links.
    pub level: String,
    /// The tail, newest-first, already level-filtered (askama HTML-escapes each).
    pub lines: Vec<String>,
    /// The file we tailed (e.g. `hotchkiss.io.log.2026-06-29`), or "" if none.
    pub source: String,
}

/// `GET /admin/logs?level=all|warn|error` — gated by the `/admin` `require_admin`
/// layer. The disk tail runs in `spawn_blocking` so a log on a slow/asleep disk
/// can't pin a tokio worker (same lesson as the media byte route, CN.12).
pub async fn show_logs(
    State(state): State<AppState>,
    session_data: SessionData,
    Query(q): Query<LogsQuery>,
) -> Result<Response, AppError> {
    let level = LogLevel::parse(q.level.as_deref());
    let dir = state.log_path.clone();
    let (lines, source) =
        tokio::task::spawn_blocking(move || read_log_tail(&dir, level)).await??;

    let tmpl = LogsTemplate {
        top_bar: TopBar::create(&state.pool, "admin", session_data.auth_state.role()).await?,
        auth_state: session_data.auth_state,
        level: level.as_str().to_string(),
        lines,
        source,
    };
    Ok(HtmlTemplate(tmpl).into_response())
}

/// Read the app-log tail newest-first, filtered to `level`, WALKING BACK across
/// tracing's DAILY-rotated `hotchkiss.io.log*` files (skipping the `access.log`
/// sibling) until `TAIL_LINES` matches are collected or `MAX_FILES` files are
/// read. Returns (lines, sources) where `sources` names each file that actually
/// contributed a line, newest-first. A missing dir / no log file is an EMPTY
/// tail, not an error.
///
/// DM.9: the pre-fix version read only the SINGLE newest file, so an ERROR that
/// had rotated into yesterday's file went invisible the moment today's file
/// filled its 256 KiB tail with newer noise — a warn/error view could read empty
/// while the incident sat one file over. Under `All` today's file usually fills
/// the budget alone (same as before); the walk-back only kicks in when the
/// filter leaves the newest file short — exactly the error-hunting case.
fn read_log_tail(dir: &Path, level: LogLevel) -> anyhow::Result<(Vec<String>, String)> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok((Vec::new(), String::new())), // dir not created yet
    };

    // Every app-log file, newest mtime first (access.log* and others skipped).
    let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("hotchkiss.io.log")
        {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        files.push((mtime, entry.path()));
    }
    files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime)); // newest first

    let mut out: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    for (_, path) in files.into_iter().take(MAX_FILES) {
        if out.len() >= TAIL_LINES {
            break;
        }
        let file_lines = read_one_tail(&path, level)?; // newest-first, filtered
        if file_lines.is_empty() {
            continue; // don't credit a file that contributed nothing
        }
        let remaining = TAIL_LINES - out.len();
        out.extend(file_lines.into_iter().take(remaining));
        if let Some(name) = path.file_name() {
            sources.push(name.to_string_lossy().into_owned());
        }
    }

    Ok((out, sources.join(", ")))
}

/// Read the last `TAIL_BYTES` of ONE file and return its lines matching `level`,
/// newest-first.
fn read_one_tail(path: &Path, level: LogLevel) -> anyhow::Result<Vec<String>> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);

    let mut lines: Vec<&str> = text.lines().collect();
    // Seeked into the middle of the file -> the first line is a partial; drop it.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Ok(lines
        .iter()
        .filter(|l| level.matches(l))
        .rev() // newest-first
        .map(|s| s.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn write_with_mtime(path: &Path, content: &str, mtime_secs: u64) {
        std::fs::write(path, content).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(mtime_secs))
            .unwrap();
    }

    #[test]
    fn tail_walks_files_newest_first_and_filters() {
        let dir = std::env::temp_dir().join(format!("hio-logtest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // an OLDER rotated file (INFO only) + an access.log sibling. The access
        // sibling is ALWAYS ignored; the older app-log is now walked into when
        // the newest file leaves the budget unfilled (DM.9).
        write_with_mtime(
            &dir.join("hotchkiss.io.log.2026-06-28"),
            "2026-06-28T00:00:00Z  INFO old: from yesterday\n",
            1_000,
        );
        write_with_mtime(&dir.join("access.log.2026-06-29"), "irrelevant access line\n", 9_000);
        // the NEWEST app log (by mtime).
        write_with_mtime(
            &dir.join("hotchkiss.io.log.2026-06-29"),
            "2026-06-29T00:00:01Z  INFO t: first\n\
             2026-06-29T00:00:02Z  WARN t: a warning\n\
             2026-06-29T00:00:03Z ERROR t: a problem\n",
            2_000,
        );

        // All: both app-log files, newest-first, older file's line last; the
        // access.log sibling is never included.
        let (all, src) = read_log_tail(&dir, LogLevel::All).unwrap();
        assert_eq!(all.len(), 4);
        assert!(all[0].contains("a problem"), "newest line first");
        assert!(all[3].contains("from yesterday"), "older file walked in, last");
        assert!(!all.iter().any(|l| l.contains("access line")), "not access.log");
        assert_eq!(src, "hotchkiss.io.log.2026-06-29, hotchkiss.io.log.2026-06-28");

        // Warn / Error: only the newest file has matches, so the older INFO-only
        // file contributes nothing and isn't credited as a source.
        let (warn, warn_src) = read_log_tail(&dir, LogLevel::Warn).unwrap();
        assert_eq!(warn.len(), 2, "warn + error");
        assert!(warn.iter().all(|l| l.contains("WARN") || l.contains("ERROR")));
        assert_eq!(warn_src, "hotchkiss.io.log.2026-06-29");

        let (err, _) = read_log_tail(&dir, LogLevel::Error).unwrap();
        assert_eq!(err.len(), 1);
        assert!(err[0].contains("a problem"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// DM.9 regression guard: an ERROR that rotated into an OLDER file must still
    /// surface when today's (newest) file has no matching line — the exact
    /// blind spot the single-newest-file tail created.
    #[test]
    fn error_in_older_file_is_found_when_newest_lacks_it() {
        let dir = std::env::temp_dir().join(format!("hio-logtest-older-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Yesterday: the ERROR we're hunting.
        write_with_mtime(
            &dir.join("hotchkiss.io.log.2026-06-28"),
            "2026-06-28T09:00:00Z ERROR boot: the incident\n",
            1_000,
        );
        // Today: only INFO — the old code would have stopped here and reported
        // an empty error view.
        write_with_mtime(
            &dir.join("hotchkiss.io.log.2026-06-29"),
            "2026-06-29T00:00:01Z  INFO t: nothing wrong today\n",
            2_000,
        );

        let (err, src) = read_log_tail(&dir, LogLevel::Error).unwrap();
        assert_eq!(err.len(), 1, "the older file's error is found");
        assert!(err[0].contains("the incident"));
        assert_eq!(src, "hotchkiss.io.log.2026-06-28", "credited to the older file");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_dir_is_empty_not_error() {
        let dir =
            std::env::temp_dir().join(format!("hio-logtest-missing-{}", uuid::Uuid::new_v4()));
        let (lines, src) = read_log_tail(&dir, LogLevel::All).unwrap();
        assert!(lines.is_empty());
        assert!(src.is_empty());
    }
}
