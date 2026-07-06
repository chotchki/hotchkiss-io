//! The detection sweep: `request_log` → per-IP features → verdict → (FCrDNS for the blunt
//! rules) → greylist upsert. Runs DETACHED on an interval (like the coordinator backfills),
//! so a failure logs and self-heals on the next tick instead of taking the app down.
//!
//! It is INERT until the enforcement middleware (CX.5) is wired — the sweep writes greylist
//! rows, but nothing serves the toll yet, so early rows have no visible effect. That's
//! deliberate: it lets detection be validated (rows appearing on beta) before the challenge
//! ships.

use std::time::Duration;

use anyhow::Result;
use hickory_resolver::TokioAsyncResolver;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tracing::{error, info, warn};

use crate::db::dao::greylist::GreylistDao;
use crate::db::dao::request_log::{RequestLogDao, Window};
use crate::greylist::active_set::GreylistSet;
use crate::greylist::crawler::{CrawlerCache, CrawlerDns, CrawlerVerdict};
use crate::greylist::detection::{build_features, score, Verdict};

/// Lookback the sweep evaluates each pass (the R2/R3 counts accumulate over this).
pub const SWEEP_WINDOW_DAYS: i64 = 1;
/// Sliding lifetime of an auto greylist entry from its last trip.
pub const GREYLIST_TTL_DAYS: i64 = 7;
/// Per-DNS-lookup timeout during FCrDNS (the sweep is off the request path, so this is generous).
pub const DNS_TIMEOUT: Duration = Duration::from_secs(3);
/// How often the detached loop runs.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Crawler-verdict cache lifetime — crawler IP ranges are stable, so hours is fine.
pub const CRAWLER_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Outcome of one sweep pass — returned so the "Run sweep now" admin action (CX.12) can report it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub evaluated: usize,
    pub greylisted: usize,
    pub exempted_crawlers: usize,
    pub skipped_unknown_dns: usize,
}

fn ttl_expiry(now: DateTime<Utc>) -> DateTime<Utc> {
    // chrono's Duration/TimeDelta isn't re-exported by sqlx, so add via timestamp math (same
    // trick as request_log::Window::last_days).
    DateTime::<Utc>::from_timestamp(now.timestamp() + GREYLIST_TTL_DAYS * 86_400, 0).unwrap_or(now)
}

/// One sweep pass. Injectable pool + resolver + cache so it tests offline. Greylists via R1
/// unconditionally (signature probes need no DNS); for the blunt rules (R2/R3) it consults
/// FCrDNS and EXEMPTS a verified crawler, SKIPS on inconclusive DNS (fail-safe), and greylists
/// a confirmed non-crawler.
pub async fn run_once<D: CrawlerDns>(
    pool: &SqlitePool,
    dns: &D,
    cache: &CrawlerCache,
    set: &GreylistSet,
) -> Result<SweepReport> {
    let window = Window::last_days(SWEEP_WINDOW_DAYS);
    let rows = RequestLogDao::ip_path_aggregates(pool, &window).await?;
    let features = build_features(&rows);

    let mut report = SweepReport {
        evaluated: features.len(),
        ..Default::default()
    };
    let expires = ttl_expiry(Utc::now());

    for f in &features {
        let Verdict::Greylist {
            rule,
            reason,
            evidence,
        } = score(f)
        else {
            continue;
        };

        if rule.exempts_verified_crawlers() {
            // `f.ip` is a public IP by construction (build_features ran should_evaluate), so the
            // parse won't fail — but if it somehow did, skip rather than misattribute.
            let Ok(ip) = f.ip.parse() else { continue };
            match cache.verify(dns, ip, DNS_TIMEOUT).await {
                CrawlerVerdict::Verified => {
                    info!("greylist sweep: exempting verified crawler {} (would-be {reason})", f.ip);
                    report.exempted_crawlers += 1;
                    continue;
                }
                CrawlerVerdict::Unknown => {
                    warn!("greylist sweep: DNS inconclusive for {} ({reason}), skipping this tick", f.ip);
                    report.skipped_unknown_dns += 1;
                    continue;
                }
                CrawlerVerdict::NotCrawler => {} // fall through to greylist
            }
        }

        GreylistDao::upsert_auto(pool, &f.ip, &reason, Some(&evidence), expires).await?;
        report.greylisted += 1;
    }

    // Trim lapsed rows, then refresh the request-path snapshot from the active set.
    let pruned = GreylistDao::prune_expired(pool).await?;
    let active = GreylistDao::active(pool).await?;
    set.refresh(&active);

    info!(
        "greylist sweep: evaluated {} IPs, greylisted {}, exempted {} crawlers, skipped {} (DNS inconclusive), pruned {}, active {}",
        report.evaluated, report.greylisted, report.exempted_crawlers, report.skipped_unknown_dns, pruned, active.len()
    );
    Ok(report)
}

/// Spawn the sweep as a detached interval loop (NOT in the coordinator `try_join!`, so a failure
/// can't take the app down — it logs and retries next tick). Runs once at boot, then every
/// [`SWEEP_INTERVAL`].
pub fn spawn(pool: SqlitePool, resolver: TokioAsyncResolver, set: GreylistSet) {
    tokio::spawn(async move {
        // Enforce persisted entries from t=0 (before the first detection pass runs).
        if let Ok(active) = GreylistDao::active(&pool).await {
            set.refresh(&active);
        }
        let cache = CrawlerCache::new(CRAWLER_CACHE_TTL);
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            ticker.tick().await; // fires immediately on the first tick, then every interval
            if let Err(e) = run_once(&pool, &resolver, &cache, &set).await {
                error!("greylist sweep pass failed (will retry next tick): {e:?}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::dao::request_log::NewRequestLog;
    use std::collections::HashMap;
    use std::net::IpAddr;

    /// Minimal offline resolver for the sweep test: verifies exactly the IPs whose PTR+forward
    /// are configured, everything else is NotCrawler.
    #[derive(Default)]
    struct MockDns {
        ptr: HashMap<IpAddr, Vec<String>>,
        fwd: HashMap<String, Vec<IpAddr>>,
    }
    impl CrawlerDns for MockDns {
        async fn ptr(&self, ip: IpAddr) -> Result<Vec<String>> {
            Ok(self.ptr.get(&ip).cloned().unwrap_or_default())
        }
        async fn forward(&self, host: &str) -> Result<Vec<IpAddr>> {
            Ok(self.fwd.get(host).cloned().unwrap_or_default())
        }
    }

    fn req(path: &str, status: i64, ip: &str) -> NewRequestLog {
        NewRequestLog {
            method: "GET".into(),
            path: path.into(),
            status,
            ip: Some(ip.into()),
            user_agent: None,
            referer: None,
            duration_ms: 0,
            is_bot: false,
            challenged: false,
        }
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn sweep_greylists_scanner_exempts_crawler_leaves_quiet(pool: SqlitePool) -> Result<()> {
        let scanner = "203.0.113.50";
        let crawler = "66.249.66.20";
        let quiet = "198.51.100.30";

        // Scanner → R1 (signature probes; no DNS consulted).
        for p in ["/wp-login.php", "/xmlrpc.php", "/.env"] {
            RequestLogDao::insert(&pool, &req(p, 404, scanner)).await?;
        }
        // Crawler → R2 only (45 distinct dead paths, zero signature hits).
        for i in 0..45 {
            RequestLogDao::insert(&pool, &req(&format!("/dead-{i}"), 404, crawler)).await?;
        }
        // Quiet legit visitor → no verdict.
        for _ in 0..3 {
            RequestLogDao::insert(&pool, &req("/pages/home", 200, quiet)).await?;
        }

        let mut dns = MockDns::default();
        let cip: IpAddr = crawler.parse().unwrap();
        dns.ptr.insert(cip, vec!["crawl-66-249-66-20.googlebot.com.".into()]);
        dns.fwd.insert("crawl-66-249-66-20.googlebot.com.".into(), vec![cip]);

        let cache = CrawlerCache::new(Duration::from_secs(3600));
        let set = GreylistSet::new();
        let report = run_once(&pool, &dns, &cache, &set).await?;

        let active = GreylistDao::active(&pool).await?;
        let ips: Vec<&str> = active.iter().map(|e| e.ip.as_str()).collect();

        assert!(ips.contains(&scanner), "scanner greylisted via R1");
        assert!(!ips.contains(&crawler), "verified crawler exempted from R2");
        assert!(!ips.contains(&quiet), "quiet visitor untouched");

        // The in-memory snapshot the request path reads was refreshed by the pass.
        assert!(set.is_greylisted(scanner), "snapshot reflects the greylist");
        assert!(!set.is_greylisted(crawler), "exempted crawler not in the snapshot");

        let scanner_row = active.iter().find(|e| e.ip == scanner).unwrap();
        assert!(scanner_row.reason.starts_with("R1"), "reason names the rule");
        assert!(!scanner_row.manual, "auto entry, not a manual pin");
        assert!(scanner_row.expires_at.is_some(), "auto entry carries a sliding expiry");

        assert_eq!(report.greylisted, 1);
        assert_eq!(report.exempted_crawlers, 1);
        assert_eq!(report.skipped_unknown_dns, 0);
        Ok(())
    }
}
