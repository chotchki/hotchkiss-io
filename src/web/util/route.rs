//! Latency route-bucketing + percentiles (CQ.6). `normalize_route` collapses a raw
//! request path to a route PATTERN so per-route latency doesn't shatter across ids /
//! slugs; the percentile helpers are Rust-side because SQLite has no percentile fn.
//!
//! `normalize_route` is a hand-maintained MIRROR of the axum router in `web/router.rs`
//! and the nested routers. DRIFT RISK: a NEW id-bearing route needs a rule here, or
//! each id buckets as its own "route" and dilutes the stats. The `route_patterns`
//! unit test pins the current mapping so a drift fails loudly.

use crate::db::dao::request_log::LatencySample;

/// Collapse a raw path to a route pattern. Longest / most-specific prefix first;
/// anything unmatched passes through raw (a leaf route with no id is already its own
/// pattern). Exact routes that share a prefix with an id route (e.g. `/blog` vs
/// `/blog/:slug`) are special-cased so they don't collapse.
pub fn normalize_route(path: &str) -> String {
    // Exact routes under an id-bearing prefix — must NOT collapse.
    if path == "/blog" || path == "/blog/feed.xml" {
        return path.to_string();
    }
    if path.strip_prefix("/blog/").is_some_and(|rest| !rest.is_empty()) {
        return "/blog/:slug".to_string();
    }

    // (prefix, pattern) — order matters, most-specific first.
    const RULES: &[(&str, &str)] = &[
        ("/media/file/", "/media/file/:key"),
        ("/media/embed/", "/media/embed/:ref"),
        ("/media/", "/media/:ref"),
        ("/diagram/", "/diagram/:hash"),
        ("/pages/", "/pages/*"),
        ("/admin/analytics/ip/", "/admin/analytics/ip/:ip"),
        ("/admin/users/", "/admin/users/:id"),
        ("/admin/api-keys/", "/admin/api-keys/:id"),
        ("/admin/media/", "/admin/media/:id"),
    ];
    for (prefix, pattern) in RULES {
        if path.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty()) {
            return pattern.to_string();
        }
    }
    path.to_string()
}

/// Nearest-rank percentile of a PRE-SORTED ascending slice. `p` in [0,100]. Empty → 0.
/// rank = ceil(p/100 · n) (1-indexed), index = rank-1 clamped to [0, n-1]. So p95 of
/// 10 items is the 10th (the max at small n), p50 of 1 item is that item.
pub fn percentile_sorted(sorted: &[i64], p: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let n = sorted.len();
    let rank = ((p / 100.0) * n as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

/// Per-route latency summary (CQ.6). `p99` is computed but NOT displayed (≈ max at
/// personal-site sample counts — column noise); it's here for the honest record.
#[derive(Clone, Debug)]
pub struct RouteLatency {
    pub route: String,
    pub count: i64,
    pub p50: i64,
    pub p95: i64,
    /// Computed for the honest record but intentionally NOT displayed (≈ max at
    /// personal-site sample counts — column noise).
    #[allow(dead_code)]
    pub p99: i64,
    pub max: i64,
}

/// Group samples by normalized route, compute percentiles per route, sort by p95 desc
/// (the bottleneck-finder ordering). Ties broken by count desc then route asc for a
/// stable display.
pub fn aggregate_by_route(samples: &[LatencySample]) -> Vec<RouteLatency> {
    use std::collections::HashMap;

    let mut by: HashMap<String, Vec<i64>> = HashMap::new();
    for s in samples {
        by.entry(normalize_route(&s.path))
            .or_default()
            .push(s.duration_ms);
    }

    let mut out: Vec<RouteLatency> = by
        .into_iter()
        .map(|(route, mut ds)| {
            ds.sort_unstable();
            let n = ds.len();
            RouteLatency {
                route,
                count: n as i64,
                p50: percentile_sorted(&ds, 50.0),
                p95: percentile_sorted(&ds, 95.0),
                p99: percentile_sorted(&ds, 99.0),
                max: ds[n - 1],
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.p95
            .cmp(&a.p95)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.route.cmp(&b.route))
    });
    out
}

/// Overall p95 across every sample — the honest headline number.
pub fn overall_p95(samples: &[LatencySample]) -> i64 {
    let mut ds: Vec<i64> = samples.iter().map(|s| s.duration_ms).collect();
    ds.sort_unstable();
    percentile_sorted(&ds, 95.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(path: &str, ms: i64) -> LatencySample {
        LatencySample {
            path: path.to_string(),
            duration_ms: ms,
        }
    }

    #[test]
    fn route_patterns() {
        // The pinned mirror — a drift here means the router grew an id route without a
        // rule (or a rule went stale).
        assert_eq!(normalize_route("/"), "/");
        assert_eq!(normalize_route("/blog"), "/blog");
        assert_eq!(normalize_route("/blog/feed.xml"), "/blog/feed.xml");
        assert_eq!(normalize_route("/blog/my-post"), "/blog/:slug");
        assert_eq!(normalize_route("/pages/projects/recon-gen"), "/pages/*");
        assert_eq!(normalize_route("/media/file/abcd1234"), "/media/file/:key");
        assert_eq!(normalize_route("/media/embed/uuid-here"), "/media/embed/:ref");
        assert_eq!(normalize_route("/media/uuid-here"), "/media/:ref");
        assert_eq!(normalize_route("/diagram/deadbeef"), "/diagram/:hash");
        assert_eq!(normalize_route("/admin/analytics/ip/1.2.3.4"), "/admin/analytics/ip/:ip");
        assert_eq!(normalize_route("/admin/analytics"), "/admin/analytics");
        assert_eq!(normalize_route("/admin/users/42"), "/admin/users/:id");
        assert_eq!(normalize_route("/admin/users/42/role"), "/admin/users/:id");
        assert_eq!(normalize_route("/resume"), "/resume");
    }

    #[test]
    fn percentile_edges() {
        assert_eq!(percentile_sorted(&[], 50.0), 0, "empty → 0");
        assert_eq!(percentile_sorted(&[7], 50.0), 7, "n=1 → that value");
        assert_eq!(percentile_sorted(&[7], 95.0), 7);
        // n=4 sorted [1,2,3,4]: p50 rank=ceil(2.0)=2→idx1→2; p95 rank=ceil(3.8)=4→idx3→4
        let even = [1, 2, 3, 4];
        assert_eq!(percentile_sorted(&even, 50.0), 2);
        assert_eq!(percentile_sorted(&even, 95.0), 4);
        // n=5 [10,20,30,40,50]: p50 rank=ceil(2.5)=3→idx2→30; p95 rank=ceil(4.75)=5→idx4→50
        let odd = [10, 20, 30, 40, 50];
        assert_eq!(percentile_sorted(&odd, 50.0), 30);
        assert_eq!(percentile_sorted(&odd, 95.0), 50);
    }

    #[test]
    fn aggregate_groups_and_sorts_by_p95() {
        // /diagram is slow, / is fast — /diagram must sort first (p95 desc).
        let mut samples = vec![sample("/", 5), sample("/", 6), sample("/", 4)];
        for _ in 0..10 {
            samples.push(sample("/diagram/abc", 300));
        }
        samples.push(sample("/blog/a", 20));
        samples.push(sample("/blog/b", 22)); // both → /blog/:slug

        let agg = aggregate_by_route(&samples);
        assert_eq!(agg[0].route, "/diagram/:hash", "slowest route sorts first");
        assert_eq!(agg[0].count, 10);
        assert_eq!(agg[0].p95, 300);

        let blog = agg.iter().find(|r| r.route == "/blog/:slug").unwrap();
        assert_eq!(blog.count, 2, "both slugs collapse into one route bucket");

        // 15 samples, ten of them 300 → p95 (rank ceil(0.95·15)=15) lands on a 300.
        assert_eq!(overall_p95(&samples), 300);
    }
}
