//! Forward-confirmed reverse DNS (FCrDNS) crawler verification (CX.3).
//!
//! Before the sweep auto-greylists an IP via a BLUNT rule (R2 404-burst / R3 flood), it
//! checks whether the IP is a real search crawler — because a real crawler can plausibly trip
//! those (Googlebot re-crawling dead URLs after a restructure). A User-Agent is no protection
//! (spoofed first), so we verify via DNS the requester can't forge: reverse-lookup the IP,
//! require the PTR to end in a known crawler suffix, THEN forward-resolve that name and confirm
//! it maps back to the same IP. R1 (signature probe) never consults this — nothing legitimate
//! probes `wp-login.php`.
//!
//! Fail-safe: a DNS error or timeout yields [`CrawlerVerdict::Unknown`], and the sweep SKIPS
//! greylisting on Unknown rather than punishing on incomplete info (the abuser almost always
//! also trips R1, which needs no DNS, so nothing slips through). The resolver is injected via
//! [`CrawlerDns`] so tests never touch the network.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use hickory_resolver::{
    error::{ResolveError, ResolveErrorKind},
    TokioAsyncResolver,
};

/// PTR suffixes that identify a real crawler operator's reverse-DNS namespace. Each carries a
/// LEADING dot so the match is on a domain boundary — `evilgooglebot.com` does NOT end with
/// `.googlebot.com`. Verification still requires the forward-confirm, so a suffix match alone
/// never exempts. (Google/Bing/Yahoo/Yandex/Baidu/Apple publish these.)
pub const CRAWLER_SUFFIXES: &[&str] = &[
    ".googlebot.com",
    ".google.com",
    ".search.msn.com",
    ".crawl.yahoo.net",
    ".yandex.com",
    ".yandex.net",
    ".yandex.ru",
    ".crawl.baidu.com",
    ".baidu.com",
    ".applebot.apple.com",
];

/// The result of an FCrDNS check. `Unknown` = the lookup failed or timed out; the caller must
/// treat it as "don't act this tick", NOT as "not a crawler".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrawlerVerdict {
    /// PTR matched a crawler suffix AND forward-resolved back to the same IP.
    Verified,
    /// No PTR, a non-crawler PTR, or a suffix match whose forward lookup did NOT confirm the IP
    /// (a spoofed PTR).
    NotCrawler,
    /// DNS error or timeout — incomplete information.
    Unknown,
}

/// The two lookups FCrDNS needs, injectable so tests run offline. Implemented for the app's
/// `TokioAsyncResolver` (the same type the ACME path uses).
#[allow(async_fn_in_trait)] // crate-internal; the concrete TokioAsyncResolver future is Send
pub trait CrawlerDns {
    /// PTR records for `ip` as FQDN strings (may carry a trailing dot). Empty = no PTR.
    async fn ptr(&self, ip: IpAddr) -> Result<Vec<String>>;
    /// A/AAAA records for `host`. Empty = no record.
    async fn forward(&self, host: &str) -> Result<Vec<IpAddr>>;
}

fn is_no_records(e: &ResolveError) -> bool {
    matches!(e.kind(), ResolveErrorKind::NoRecordsFound { .. })
}

impl CrawlerDns for TokioAsyncResolver {
    async fn ptr(&self, ip: IpAddr) -> Result<Vec<String>> {
        match self.reverse_lookup(ip).await {
            Ok(r) => Ok(r.iter().map(|name| name.to_string()).collect()),
            Err(ref e) if is_no_records(e) => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    async fn forward(&self, host: &str) -> Result<Vec<IpAddr>> {
        match self.lookup_ip(host.to_string()).await {
            Ok(r) => Ok(r.iter().collect()),
            Err(ref e) if is_no_records(e) => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }
}

/// True if `name` (trailing dot / case ignored) falls under a crawler suffix on a domain boundary.
fn matches_crawler_suffix(name: &str) -> bool {
    let n = name.trim_end_matches('.').to_ascii_lowercase();
    CRAWLER_SUFFIXES.iter().any(|s| n.ends_with(s))
}

/// One FCrDNS verification. `timeout` bounds EACH lookup. Never panics; a failure is `Unknown`.
pub async fn verify_crawler<D: CrawlerDns>(
    dns: &D,
    ip: IpAddr,
    timeout: Duration,
) -> CrawlerVerdict {
    let names = match tokio::time::timeout(timeout, dns.ptr(ip)).await {
        Ok(Ok(names)) => names,
        Ok(Err(_)) | Err(_) => return CrawlerVerdict::Unknown, // DNS error OR timeout
    };

    let matched: Vec<&String> = names.iter().filter(|n| matches_crawler_suffix(n)).collect();
    if matched.is_empty() {
        // No PTR at all, or a PTR that isn't a crawler namespace — not a crawler (and not a
        // DNS failure, so it's safe to greylist).
        return CrawlerVerdict::NotCrawler;
    }

    for name in matched {
        match tokio::time::timeout(timeout, dns.forward(name)).await {
            Ok(Ok(ips)) if ips.contains(&ip) => return CrawlerVerdict::Verified,
            Ok(Ok(_)) => continue, // this name didn't confirm; try the next matched PTR
            Ok(Err(_)) | Err(_) => return CrawlerVerdict::Unknown, // forward failed → incomplete
        }
    }
    // A crawler-suffix PTR that forward-resolves to a DIFFERENT IP is a SPOOFED PTR — not a
    // real crawler, so it's fair game for greylisting.
    CrawlerVerdict::NotCrawler
}

/// A TTL memo over [`verify_crawler`] so repeated sweeps don't re-resolve the same IP. Only
/// definitive verdicts are cached; `Unknown` is never cached (retried next tick). Crawler IP
/// ranges are stable, so a multi-hour TTL is fine.
pub struct CrawlerCache {
    ttl: Duration,
    // ip -> (is_verified_crawler, cached_at)
    inner: Mutex<HashMap<IpAddr, (bool, Instant)>>,
}

impl CrawlerCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn cached(&self, ip: IpAddr, now: Instant) -> Option<bool> {
        let guard = self.inner.lock().unwrap();
        guard
            .get(&ip)
            .filter(|(_, at)| now.duration_since(*at) < self.ttl)
            .map(|(v, _)| *v)
    }

    fn store(&self, ip: IpAddr, verified: bool, now: Instant) {
        self.inner.lock().unwrap().insert(ip, (verified, now));
    }

    /// Memoized verification. Returns `Verified`/`NotCrawler` from cache when fresh; otherwise
    /// resolves, caches a definitive verdict, and leaves `Unknown` uncached.
    pub async fn verify<D: CrawlerDns>(
        &self,
        dns: &D,
        ip: IpAddr,
        timeout: Duration,
    ) -> CrawlerVerdict {
        let now = Instant::now();
        if let Some(v) = self.cached(ip, now) {
            return if v {
                CrawlerVerdict::Verified
            } else {
                CrawlerVerdict::NotCrawler
            };
        }
        let verdict = verify_crawler(dns, ip, timeout).await;
        match verdict {
            CrawlerVerdict::Verified => self.store(ip, true, now),
            CrawlerVerdict::NotCrawler => self.store(ip, false, now),
            CrawlerVerdict::Unknown => {}
        }
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    /// Offline resolver. `None` in a map = an empty (NoRecords) answer; `Some(Err)` = a resolver
    /// error. `delay` lets a test exercise the per-lookup timeout under paused time.
    #[derive(Default)]
    struct MockDns {
        ptr: HashMap<IpAddr, std::result::Result<Vec<String>, ()>>,
        fwd: HashMap<String, std::result::Result<Vec<IpAddr>, ()>>,
        delay: Duration,
    }

    impl CrawlerDns for MockDns {
        async fn ptr(&self, ip: IpAddr) -> Result<Vec<String>> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            match self.ptr.get(&ip) {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(())) => Err(anyhow!("simulated PTR failure")),
                None => Ok(vec![]),
            }
        }
        async fn forward(&self, host: &str) -> Result<Vec<IpAddr>> {
            match self.fwd.get(host) {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(())) => Err(anyhow!("simulated forward failure")),
                None => Ok(vec![]),
            }
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    const T: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn verified_when_ptr_matches_and_forward_confirms() {
        let g = ip("66.249.66.1");
        let mut m = MockDns::default();
        m.ptr.insert(g, Ok(vec!["crawl-66-249-66-1.googlebot.com.".into()]));
        m.fwd.insert("crawl-66-249-66-1.googlebot.com.".into(), Ok(vec![g]));
        assert_eq!(verify_crawler(&m, g, T).await, CrawlerVerdict::Verified);
    }

    #[tokio::test]
    async fn spoofed_ptr_forward_mismatch_is_not_a_crawler() {
        // Attacker sets a googlebot.com PTR, but the forward lookup points elsewhere.
        let evil = ip("203.0.113.9");
        let mut m = MockDns::default();
        m.ptr.insert(evil, Ok(vec!["crawl.googlebot.com.".into()]));
        m.fwd.insert("crawl.googlebot.com.".into(), Ok(vec![ip("66.249.66.1")]));
        assert_eq!(verify_crawler(&m, evil, T).await, CrawlerVerdict::NotCrawler);
    }

    #[tokio::test]
    async fn evil_prefix_does_not_match_suffix() {
        let evil = ip("203.0.113.10");
        let mut m = MockDns::default();
        m.ptr.insert(evil, Ok(vec!["host.evilgooglebot.com.".into()]));
        // No forward is even attempted (suffix didn't match on a boundary).
        assert_eq!(verify_crawler(&m, evil, T).await, CrawlerVerdict::NotCrawler);
    }

    #[tokio::test]
    async fn non_crawler_ptr_is_not_a_crawler() {
        let h = ip("198.51.100.5");
        let mut m = MockDns::default();
        m.ptr.insert(h, Ok(vec!["mail.example.com.".into()]));
        assert_eq!(verify_crawler(&m, h, T).await, CrawlerVerdict::NotCrawler);
    }

    #[tokio::test]
    async fn no_ptr_is_not_a_crawler() {
        // A bare scanner IP with no reverse record.
        assert_eq!(
            verify_crawler(&MockDns::default(), ip("185.177.72.70"), T).await,
            CrawlerVerdict::NotCrawler
        );
    }

    #[tokio::test]
    async fn ptr_error_is_unknown() {
        let x = ip("192.0.2.1");
        let mut m = MockDns::default();
        m.ptr.insert(x, Err(()));
        assert_eq!(verify_crawler(&m, x, T).await, CrawlerVerdict::Unknown);
    }

    #[tokio::test]
    async fn forward_error_is_unknown() {
        let x = ip("66.249.66.2");
        let mut m = MockDns::default();
        m.ptr.insert(x, Ok(vec!["crawl.googlebot.com.".into()]));
        m.fwd.insert("crawl.googlebot.com.".into(), Err(()));
        assert_eq!(verify_crawler(&m, x, T).await, CrawlerVerdict::Unknown);
    }

    #[tokio::test]
    async fn timeout_is_unknown() {
        let x = ip("66.249.66.3");
        let mut m = MockDns::default();
        m.ptr.insert(x, Ok(vec!["crawl.googlebot.com.".into()]));
        m.delay = Duration::from_secs(30); // far exceeds the timeout; cancelled when it fires
        // Real (tiny) timeout — the mock's 30s sleep is dropped the instant timeout wins, so
        // the test resolves in ~10ms, not 30s.
        assert_eq!(
            verify_crawler(&m, x, Duration::from_millis(10)).await,
            CrawlerVerdict::Unknown
        );
    }

    #[tokio::test]
    async fn cache_memoizes_definitive_verdicts_not_unknown() {
        let g = ip("66.249.66.1");
        let mut verified = MockDns::default();
        verified.ptr.insert(g, Ok(vec!["crawl.googlebot.com.".into()]));
        verified.fwd.insert("crawl.googlebot.com.".into(), Ok(vec![g]));

        let cache = CrawlerCache::new(Duration::from_secs(3600));
        assert_eq!(cache.verify(&verified, g, T).await, CrawlerVerdict::Verified);

        // A resolver that would now FAIL still returns the cached Verified (no re-resolve).
        let mut failing = MockDns::default();
        failing.ptr.insert(g, Err(()));
        assert_eq!(cache.verify(&failing, g, T).await, CrawlerVerdict::Verified);

        // Unknown is never cached: an uncached IP that errors stays Unknown and re-resolves.
        let u = ip("192.0.2.2");
        let mut err = MockDns::default();
        err.ptr.insert(u, Err(()));
        assert_eq!(cache.verify(&err, u, T).await, CrawlerVerdict::Unknown);
        // Now it resolves cleanly → NotCrawler (proves the Unknown wasn't cached).
        assert_eq!(cache.verify(&MockDns::default(), u, T).await, CrawlerVerdict::NotCrawler);
    }
}
