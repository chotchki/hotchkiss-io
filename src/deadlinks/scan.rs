//! DL.6 — the scan orchestration + the daily coordinator loop + the shared handle.
//!
//! `run_scan` ties it together: enumerate every content page, extract + classify its
//! links, refresh the page↔url refs, then check each DISTINCT url (internal in-DB,
//! external over HTTP with per-host politeness), persist via the confirm-before-alarm
//! streak, and prune orphans. The daily loop + the admin "Run scan now" both go
//! through `run_guarded` (single-flight via `DeadLinkScanState`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use serde::Serialize;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use super::class::{CheckClass, LinkKind};
use super::classify::classify;
use super::dao::{LinkCheckDao, LinkCheckRow, LinkRefDao, RETAIN_DAYS};
use super::extract::extract_links;
use super::external::{ExternalChecker, ReqwestChecker};
#[cfg(test)]
use super::external::CheckOutcome;
use super::internal::{resolve_internal, InternalVerdict};
use super::classify::LinkTarget;
use crate::db::dao::content_pages::ContentPageDao;

/// How often the background loop scans. Daily — link rot is slow.
pub const DEAD_LINK_SCAN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Hosts checked concurrently. Low, so the scan never hammers the wider web.
const MAX_CONCURRENT_HOSTS: usize = 4;

/// Politeness delay between two requests to the SAME host.
const PER_HOST_DELAY: Duration = Duration::from_millis(250);

/// The tally of one scan pass, shown on the admin header.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanSummary {
    pub pages_scanned: usize,
    pub links_checked: usize,
    pub confirmed_dead: usize,
    pub failing: usize,
    pub needs_review: usize,
}

/// Shared runtime handle (mirrors greylist's `GreylistSet`): a single-flight guard
/// so the daily tick and a manual trigger can't overlap, plus the last-run status
/// the admin page shows. Cloned coordinator→loop + coordinator→AppState.
#[derive(Clone, Default, Debug)]
pub struct DeadLinkScanState {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default, Debug)]
struct Inner {
    running: bool,
    last_started: Option<DateTime<Utc>>,
    last_finished: Option<DateTime<Utc>>,
    last_summary: Option<ScanSummary>,
}

/// A snapshot of the scanner's state for the admin view.
#[derive(Debug, Clone)]
pub struct DeadLinkStatus {
    pub running: bool,
    pub last_started: Option<DateTime<Utc>>,
    pub last_finished: Option<DateTime<Utc>>,
    pub last_summary: Option<ScanSummary>,
}

impl DeadLinkScanState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Claim the single-flight slot. `true` = you may scan (marked running);
    /// `false` = a scan is already in flight, do nothing.
    pub fn try_begin(&self, now: DateTime<Utc>) -> bool {
        let mut g = self.inner.lock().expect("dead-link scan lock");
        if g.running {
            return false;
        }
        g.running = true;
        g.last_started = Some(now);
        true
    }

    /// Release the slot. A `Some` summary is retained for display; an error pass
    /// passes `None` (keeps the previous good summary).
    pub fn finish(&self, now: DateTime<Utc>, summary: Option<ScanSummary>) {
        let mut g = self.inner.lock().expect("dead-link scan lock");
        g.running = false;
        g.last_finished = Some(now);
        if summary.is_some() {
            g.last_summary = summary;
        }
    }

    pub fn status(&self) -> DeadLinkStatus {
        let g = self.inner.lock().expect("dead-link scan lock");
        DeadLinkStatus {
            running: g.running,
            last_started: g.last_started,
            last_finished: g.last_finished,
            last_summary: g.last_summary.clone(),
        }
    }
}

/// The full scan pass. Generic over `ExternalChecker` so tests run offline.
pub async fn run_scan<C: ExternalChecker>(
    pool: &SqlitePool,
    checker: &C,
    site_host: &str,
    now: DateTime<Utc>,
) -> Result<ScanSummary> {
    // 1. Enumerate every content page; refresh its refs; collect the distinct
    //    (raw url → target) set to check.
    let pages = all_pages(pool).await?;
    let pages_scanned = pages.len();
    let mut targets: HashMap<String, LinkTarget> = HashMap::new();
    for page in &pages {
        let mut page_urls: Vec<String> = Vec::new();
        for raw in extract_links(&page.page_markdown) {
            let target = classify(&raw, site_host);
            if target.kind().is_some() {
                page_urls.push(raw.clone());
                targets.entry(raw).or_insert(target);
            }
        }
        LinkRefDao::replace_for_page(pool, page.page_id, &page_urls).await?;
    }

    // 2. Partition internal vs external.
    let mut internal: Vec<(String, String)> = Vec::new(); // (raw url, resolvable path)
    let mut external: Vec<String> = Vec::new();
    for (url, target) in targets {
        match target {
            LinkTarget::Internal(path) => internal.push((url, path)),
            LinkTarget::External(u) => external.push(u),
            LinkTarget::Skip(_) => {}
        }
    }
    let links_checked = internal.len() + external.len();

    // 3. Internal — DB-only, sequential (fast, no politeness needed).
    for (url, path) in internal {
        let verdict = resolve_internal(pool, &path).await?;
        LinkCheckDao::record(
            pool,
            &url,
            LinkKind::Internal,
            CheckClass::from(verdict),
            None,
            internal_detail(verdict),
            now,
        )
        .await?;
    }

    // 4. External — grouped by host: hosts concurrent (capped), each host's urls
    //    sequential with a politeness delay.
    let mut by_host: HashMap<String, Vec<String>> = HashMap::new();
    for url in external {
        by_host.entry(host_of(&url)).or_default().push(url);
    }
    stream::iter(by_host)
        .for_each_concurrent(MAX_CONCURRENT_HOSTS, |(_host, urls)| async move {
            for (i, url) in urls.iter().enumerate() {
                if i > 0 {
                    tokio::time::sleep(PER_HOST_DELAY).await;
                }
                let outcome = checker.check(url).await;
                if let Err(e) = LinkCheckDao::record(
                    pool,
                    url,
                    LinkKind::External,
                    outcome.class,
                    outcome.status,
                    &outcome.detail,
                    now,
                )
                .await
                {
                    tracing::error!("dead-link record failed for {url}: {e:?}");
                }
            }
        })
        .await;

    // 5. Prune link_check rows no page references any more.
    LinkCheckDao::prune_orphans(pool, RETAIN_DAYS).await?;

    // 6. Summarize from current DB state.
    let problems = LinkCheckDao::problem_rows(pool).await?;
    Ok(ScanSummary {
        pages_scanned,
        links_checked,
        confirmed_dead: problems.iter().filter(|r| r.is_confirmed_dead()).count(),
        failing: problems.iter().filter(|r| r.is_failing()).count(),
        needs_review: problems.iter().filter(|r| r.needs_review()).count(),
    })
}

/// Re-check ONE url synchronously (the admin per-link re-check — bounded by the
/// request timeout, so it's fine to await). Errors if the url isn't checkable.
pub async fn recheck_one<C: ExternalChecker>(
    pool: &SqlitePool,
    checker: &C,
    site_host: &str,
    url: &str,
    now: DateTime<Utc>,
) -> Result<LinkCheckRow> {
    let (kind, class, status, detail): (LinkKind, CheckClass, Option<u16>, String) =
        match classify(url, site_host) {
            LinkTarget::Internal(path) => {
                let v = resolve_internal(pool, &path).await?;
                (
                    LinkKind::Internal,
                    CheckClass::from(v),
                    None,
                    internal_detail(v).to_string(),
                )
            }
            LinkTarget::External(u) => {
                let o = checker.check(&u).await;
                (LinkKind::External, o.class, o.status, o.detail)
            }
            LinkTarget::Skip(reason) => {
                anyhow::bail!("not a checkable link ({})", reason.as_str());
            }
        };
    LinkCheckDao::record(pool, url, kind, class, status, &detail, now).await
}

/// Spawn the DETACHED daily loop (never in the coordinator `try_join!`, so a bad
/// pass can't take the app down). Builds one reused checker, ticks daily.
pub fn spawn(pool: SqlitePool, site_host: String, scanner: DeadLinkScanState) {
    tokio::spawn(async move {
        let checker = match ReqwestChecker::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("dead-link checker client build failed; scans disabled: {e:?}");
                return;
            }
        };
        let mut ticker = tokio::time::interval(DEAD_LINK_SCAN_INTERVAL);
        loop {
            ticker.tick().await; // fires immediately, then every interval
            run_guarded(&pool, &checker, &site_host, &scanner).await;
        }
    });
}

/// Fire-and-return a manual scan (the admin "Run scan now"). A full scan does
/// external HTTP + can take minutes, so we SPAWN it and let the page poll for the
/// result — no-op if a scan is already running (the guard).
pub fn trigger_now(pool: SqlitePool, site_host: String, scanner: DeadLinkScanState) {
    tokio::spawn(async move {
        let checker = match ReqwestChecker::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("dead-link checker client build failed: {e:?}");
                return;
            }
        };
        run_guarded(&pool, &checker, &site_host, &scanner).await;
    });
}

/// The single-flight wrapper both the loop + the manual trigger use.
async fn run_guarded<C: ExternalChecker>(
    pool: &SqlitePool,
    checker: &C,
    site_host: &str,
    scanner: &DeadLinkScanState,
) {
    let now = Utc::now();
    if !scanner.try_begin(now) {
        tracing::info!("dead-link scan already running; skipping this trigger");
        return;
    }
    let result = run_scan(pool, checker, site_host, now).await;
    let finished = Utc::now();
    match result {
        Ok(summary) => {
            tracing::info!("dead-link scan complete: {summary:?}");
            scanner.finish(finished, Some(summary));
        }
        Err(e) => {
            tracing::error!("dead-link scan failed (will retry next tick): {e:?}");
            scanner.finish(finished, None);
        }
    }
}

/// Every content page, tree-walked from the roots.
async fn all_pages(pool: &SqlitePool) -> Result<Vec<ContentPageDao>> {
    let mut out = Vec::new();
    let mut stack: Vec<Option<i64>> = vec![None];
    while let Some(parent) = stack.pop() {
        for child in ContentPageDao::find_by_parent(pool, parent).await? {
            stack.push(Some(child.page_id));
            out.push(child);
        }
    }
    Ok(out)
}

fn internal_detail(verdict: InternalVerdict) -> &'static str {
    match verdict {
        InternalVerdict::Ok => "resolved",
        InternalVerdict::Dead => "no such page or media",
        InternalVerdict::Unknown => "unrecognized internal route",
    }
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::types::chrono::TimeZone;

    /// Forces a class per url so run_scan is deterministic + offline.
    struct StubChecker(HashMap<String, CheckClass>);
    impl ExternalChecker for StubChecker {
        async fn check(&self, url: &str) -> CheckOutcome {
            let class = self.0.get(url).copied().unwrap_or(CheckClass::Ok);
            CheckOutcome {
                class,
                status: Some(200),
                detail: "stub".into(),
            }
        }
    }

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn scan_records_internal_and_external(pool: SqlitePool) {
        // A page linking: a live internal page, a dead internal target, a "good"
        // external, and a "dead" external.
        ContentPageDao::create(&pool, None, "about".to_string(), None, "# About".to_string(), None)
            .await
            .unwrap();
        let md = "\
[live](/pages/about)
[dead-internal](/pages/ghost)
[good-ext](https://good.example/)
[dead-ext](https://dead.example/)
[mail](mailto:x@y.z)
";
        ContentPageDao::create(&pool, None, "post".to_string(), None, md.to_string(), None)
            .await
            .unwrap();

        let stub = StubChecker(HashMap::from([
            ("https://good.example/".to_string(), CheckClass::Ok),
            ("https://dead.example/".to_string(), CheckClass::Dead),
        ]));

        let summary = run_scan(&pool, &stub, "hotchkiss.io", at(0)).await.unwrap();
        // The two authored pages PLUS the migration-seeded special pages (blog,
        // projects, resume, …) whose redirect-target markdown carries no links.
        assert!(summary.pages_scanned >= 2, "scanned {}", summary.pages_scanned);
        // 4 checkable links from the post (mailto is skipped; special pages add none).
        assert_eq!(summary.links_checked, 4);

        // The dead internal + dead external are recorded dead (day 1, not yet confirmed).
        let ghost = LinkCheckDao::get(&pool, "/pages/ghost").await.unwrap().unwrap();
        assert_eq!(ghost.class(), CheckClass::Dead);
        assert_eq!(ghost.kind(), LinkKind::Internal);
        let dead_ext = LinkCheckDao::get(&pool, "https://dead.example/").await.unwrap().unwrap();
        assert_eq!(dead_ext.class(), CheckClass::Dead);
        assert_eq!(dead_ext.kind(), LinkKind::External);

        // The live ones are ok and NOT in the problem set.
        assert_eq!(
            LinkCheckDao::get(&pool, "/pages/about").await.unwrap().unwrap().class(),
            CheckClass::Ok
        );
        assert_eq!(summary.failing, 2, "two dead links, not yet confirmed");
        assert_eq!(summary.confirmed_dead, 0);

        // The mailto was skipped — never recorded.
        assert!(LinkCheckDao::get(&pool, "mailto:x@y.z").await.unwrap().is_none());
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn three_scans_confirm_a_dead_link(pool: SqlitePool) {
        ContentPageDao::create(&pool, None, "p".to_string(), None, "[d](https://dead.example/)".to_string(), None)
            .await
            .unwrap();
        let stub = StubChecker(HashMap::from([("https://dead.example/".to_string(), CheckClass::Dead)]));
        const DAY: i64 = 24 * 3600;
        let mut summary = ScanSummary::default();
        for d in 0..3 {
            summary = run_scan(&pool, &stub, "hotchkiss.io", at(d * DAY)).await.unwrap();
        }
        assert_eq!(summary.confirmed_dead, 1, "3 daily dead passes → confirmed");
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn refs_track_the_referencing_page(pool: SqlitePool) {
        let page = ContentPageDao::create(&pool, None, "p".to_string(), None, "[d](https://dead.example/)".to_string(), None)
            .await
            .unwrap();
        let stub = StubChecker(HashMap::from([("https://dead.example/".to_string(), CheckClass::Dead)]));
        run_scan(&pool, &stub, "hotchkiss.io", at(0)).await.unwrap();
        assert_eq!(
            LinkRefDao::pages_for_url(&pool, "https://dead.example/").await.unwrap(),
            vec![page.page_id]
        );
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn single_flight_guard_blocks_overlap(pool: SqlitePool) {
        let scanner = DeadLinkScanState::new();
        assert!(scanner.try_begin(at(0)), "first claim succeeds");
        assert!(!scanner.try_begin(at(1)), "second claim blocked while running");
        scanner.finish(at(2), Some(ScanSummary::default()));
        assert!(scanner.try_begin(at(3)), "claimable again after finish");
        let _ = &pool;
    }
}
