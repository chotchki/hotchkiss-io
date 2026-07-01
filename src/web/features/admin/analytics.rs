use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::NaiveDate;

use crate::{
    db::dao::request_log::{
        Audience, AudienceCounts, DayCount, IpPathStatus, NoisyIp, PathCount, RequestLogDao,
        StatusBucketCounts, UserAgentCount, SCAN_DISTINCT_404_THRESHOLD,
    },
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate, session::SessionData,
        util::{
            referer::{group_referers, GroupedReferers},
            route::{aggregate_by_route, overall_p95, RouteLatency},
        },
    },
};

#[derive(Deserialize)]
pub struct AnalyticsQuery {
    pub since: Option<i64>,
    pub paths: Option<String>,
    /// "humans" | "bots" | anything-else→All. A String (not a typed enum) so a
    /// bad value degrades to All instead of a deserialize-reject → 500 (CQ.2).
    pub audience: Option<String>,
}

/// Two aligned daily series (CQ.7) for the d3 line chart, serialized into the page as
/// a JSON island. NUMERIC only — `days` are DB `substr(ts,1,10)` date strings, never
/// attacker-controlled — so the island can't carry a `</script>` breakout.
#[derive(Serialize, Debug, Default)]
pub struct TimeSeries {
    pub days: Vec<String>,
    pub total: Vec<i64>,
    pub unique: Vec<i64>,
    pub empty: bool,
}

#[derive(Template)]
#[template(path = "analytics/dashboard.html")]
pub struct AnalyticsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub since_days: i64,
    /// "content" (status < 400) or "all" (incl. 4xx/5xx scanner probes) — drives
    /// the Top Pages list + which toggle chip is active.
    pub paths_mode: String,
    /// "all" | "humans" | "bots" — the active audience chip (CQ.2). The headline
    /// numbers + chart + top-pages are bucketed to this; `audience_counts` shows all
    /// three at once (the honesty display) regardless.
    pub audience: String,
    pub audience_counts: AudienceCounts,
    pub total_requests: i64,
    pub distinct_ips: i64,
    /// The two-series time-series serialized as a JSON island for the d3 line chart.
    pub ts_json: String,
    pub by_path: Vec<PathCount>,
    /// Status-code breakdown — the FACTUAL axis (CQ.3), audience-bucketed.
    pub status_buckets: StatusBucketCounts,
    /// Per-IP "who's scanning me" leaderboard (CQ.3), volume-sorted, scanner-badged.
    pub noisy_ips: Vec<NoisyIp>,
    /// Paths that only ever errored — the scanner-signature list (CQ.3).
    pub never_succeeded: Vec<PathCount>,
    pub by_user_agent: Vec<UserAgentCount>,
    /// Host-grouped external referrers + category chips + noise/direct counts (CQ.5).
    /// Directional only (spoofable / often stripped).
    pub referers: GroupedReferers,
    pub recent: Vec<RequestLogDao>,
    /// Latency (CQ.6) — SERVER-handler time, NOT client page-load/LCP.
    /// `has_latency` is false when nothing is timed yet (fresh boot / beta scrub /
    /// legacy NULL rows) → the section renders a "no timing data" note.
    pub has_latency: bool,
    pub latency_p95: i64,
    pub slow_routes: Vec<RouteLatency>,
    pub slow_requests: Vec<RequestLogDao>,
}

/// `GET /admin/analytics` — gated by the `require_admin` layer on the `admin`
/// router, so no auth check here. `?since=<days>` (default 30) sets the window;
/// `?metric=total|unique` (default total) toggles the chart series.
pub async fn show_analytics(
    State(state): State<AppState>,
    session_data: SessionData,
    Query(q): Query<AnalyticsQuery>,
) -> Result<Response, AppError> {
    let since_days = q.since.unwrap_or(30).clamp(1, 365);
    // Top-pages filter: "content" (successful loads only) hides the 404 scanner
    // probes; "all" surfaces them (status ceiling raised) so chris can see what's
    // attacking the site. Static assets are excluded either way.
    let paths_mode = if q.paths.as_deref() == Some("all") {
        "all"
    } else {
        "content"
    };
    let max_status = if paths_mode == "all" { 10_000 } else { 400 };
    // Bad ?audience → All (never a deserialize-reject/500). The headline numbers +
    // chart + top-pages bucket to this; the 3-chip below shows all three regardless.
    let audience = Audience::parse(q.audience.as_deref());

    let total_requests = RequestLogDao::count_since(&state.pool, since_days, audience).await?;
    let distinct_ips = RequestLogDao::distinct_ip_count(&state.pool, since_days, audience).await?;
    let audience_counts = RequestLogDao::audience_counts(&state.pool, since_days).await?;
    // Both daily series for the overlay chart (CQ.7) — the gap between total views and
    // unique visitors is the repeat/scanner signal. shape_timeseries zero-fills a
    // continuous UTC daily axis; the result serializes to the numeric JSON island.
    let total_by_day = RequestLogDao::count_by_day(&state.pool, since_days, audience).await?;
    let unique_by_day =
        RequestLogDao::distinct_ip_by_day(&state.pool, since_days, audience).await?;
    let timeseries = shape_timeseries(&total_by_day, &unique_by_day);
    let ts_json = serde_json::to_string(&timeseries)
        .unwrap_or_else(|_| r#"{"days":[],"total":[],"unique":[],"empty":true}"#.to_string())
        // Belt-and-suspenders XSS guard: escape any `<` so the island can never carry a
        // `</script>` (the data is numeric so this is a no-op today, but keep it honest).
        .replace('<', "\\u003c");
    let by_path =
        RequestLogDao::count_by_content_path(&state.pool, since_days, audience, max_status, 25)
            .await?;
    // Status / noise (CQ.3): the factual status breakdown, the volume-sorted per-IP
    // leaderboard (min_distinct_404=0 → show everyone; the scanner badge is a Rust
    // flag), and the never-succeeded scanner-signature paths.
    let status_buckets =
        RequestLogDao::count_by_status_bucket(&state.pool, since_days, audience).await?;
    let noisy_ips =
        RequestLogDao::noisy_ips(&state.pool, &format!("-{since_days} days"), 0, 25).await?;
    let never_succeeded = RequestLogDao::never_succeeded_paths(&state.pool, since_days, 25).await?;
    let by_user_agent = RequestLogDao::count_by_user_agent(&state.pool, since_days, 25).await?;
    // Referers (CQ.5): pull ALL distinct non-null referers + the direct (NULL) count,
    // then host-group + classify + tally noise in Rust against the canonical site host.
    let referer_urls = RequestLogDao::referer_urls_since(&state.pool, since_days).await?;
    let direct_count = RequestLogDao::direct_referer_count(&state.pool, since_days).await?;
    let referers = group_referers(&referer_urls, &state.site_host, direct_count);
    let recent = RequestLogDao::recent(&state.pool, 50).await?;

    // Latency (CQ.6): pull the windowed timed samples, aggregate per normalized route
    // + the overall p95 in Rust (SQLite has no percentile fn). `has_latency` gates the
    // empty-state (nothing timed yet on a fresh boot / after a beta scrub).
    let latency_samples = RequestLogDao::latency_samples(&state.pool, since_days).await?;
    let has_latency = !latency_samples.is_empty();
    let latency_p95 = overall_p95(&latency_samples);
    let mut slow_routes = aggregate_by_route(&latency_samples);
    slow_routes.truncate(15);
    let slow_requests = RequestLogDao::slowest_requests(&state.pool, since_days, 15).await?;

    let tmpl = AnalyticsTemplate {
        top_bar: TopBar::create(&state.pool, "admin").await?,
        auth_state: session_data.auth_state,
        since_days,
        paths_mode: paths_mode.to_string(),
        audience: audience.as_tag().to_string(),
        audience_counts,
        total_requests,
        distinct_ips,
        ts_json,
        by_path,
        status_buckets,
        noisy_ips,
        never_succeeded,
        by_user_agent,
        referers,
        recent,
        has_latency,
        latency_p95,
        slow_routes,
        slow_requests,
    };
    Ok(HtmlTemplate(tmpl).into_response())
}

#[derive(Template)]
#[template(path = "analytics/ip_detail.html")]
pub struct IpDetailTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub ip: String,
    pub since_days: i64,
    pub total: i64,
    pub distinct_paths: i64,
    /// Size of the 404 wordlist — the scan fingerprint.
    pub distinct_404: i64,
    pub errors: i64,
    /// `distinct_404 >= SCAN_DISTINCT_404_THRESHOLD` — the same heuristic the
    /// leaderboard badges (INFERRED, not authoritative).
    pub is_scanner: bool,
    pub status_buckets: StatusBucketCounts,
    /// The distinct 404 paths this IP probed (the wordlist), sorted.
    pub wordlist: Vec<String>,
    pub path_status: Vec<IpPathStatus>,
    pub user_agents: Vec<UserAgentCount>,
    pub recent: Vec<RequestLogDao>,
}

/// `GET /admin/analytics/ip/{ip}` — per-IP drill-down (CQ.4), gated by the `/admin`
/// nest's `require_admin`. A garbage `ip` segment is a clean 400 (never a DB probe or
/// a 500); an IP with no rows renders a 200 empty-state. The header stats, status mix,
/// and the 404 wordlist all derive in Rust from ONE (path,status) query. Attacker-
/// controlled path/UA strings stay in auto-escaped askama tables — the XSS boundary.
pub async fn show_ip_detail(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(ip): Path<String>,
) -> Result<Response, AppError> {
    if ip.parse::<std::net::IpAddr>().is_err() {
        return Ok((StatusCode::BAD_REQUEST, "Not a valid IP address").into_response());
    }
    // The retention window — show everything we still have for this IP.
    let since_days = 90;

    let path_status = RequestLogDao::ip_path_status(&state.pool, &ip, since_days).await?;
    let user_agents = RequestLogDao::ip_user_agents(&state.pool, &ip, since_days).await?;
    let recent = RequestLogDao::ip_recent(&state.pool, &ip, 100).await?;

    // Derive the header, status mix, and 404 wordlist from the one query — no extra
    // round-trips.
    let mut total = 0i64;
    let mut errors = 0i64;
    let mut paths = std::collections::BTreeSet::new();
    let mut wordlist = std::collections::BTreeSet::new();
    let mut sb = StatusBucketCounts {
        s2xx: 0,
        s3xx: 0,
        s403: 0,
        s404: 0,
        s4xx: 0,
        s5xx: 0,
    };
    for r in &path_status {
        total += r.count;
        if r.status >= 400 {
            errors += r.count;
        }
        paths.insert(r.path.as_str());
        if r.status == 404 {
            wordlist.insert(r.path.clone());
        }
        match r.status {
            200..=299 => sb.s2xx += r.count,
            300..=399 => sb.s3xx += r.count,
            403 => sb.s403 += r.count,
            404 => sb.s404 += r.count,
            400..=499 => sb.s4xx += r.count,
            500..=599 => sb.s5xx += r.count,
            _ => {}
        }
    }
    let distinct_paths = paths.len() as i64;
    let distinct_404 = wordlist.len() as i64;

    let tmpl = IpDetailTemplate {
        top_bar: TopBar::create(&state.pool, "admin").await?,
        auth_state: session_data.auth_state,
        ip,
        since_days,
        total,
        distinct_paths,
        distinct_404,
        errors,
        is_scanner: distinct_404 >= SCAN_DISTINCT_404_THRESHOLD,
        status_buckets: sb,
        wordlist: wordlist.into_iter().collect(),
        path_status,
        user_agents,
        recent,
    };
    Ok(HtmlTemplate(tmpl).into_response())
}

/// Build the two-series daily time series for the d3 line chart (CQ.7). Zero-fills a
/// CONTINUOUS UTC daily axis between the first and last day that carries traffic —
/// interior no-traffic days become a literal 0 (real data), NOT a null gap. Both
/// series align to the same day axis. Empty input → `empty: true` (the renderer draws
/// a note). If a day string somehow won't parse, falls back to the sparse present-days
/// (no gap-fill) rather than dropping the chart.
fn shape_timeseries(total: &[DayCount], unique: &[DayCount]) -> TimeSeries {
    use std::collections::HashMap;

    let total_map: HashMap<&str, i64> = total.iter().map(|d| (d.day.as_str(), d.count)).collect();
    let unique_map: HashMap<&str, i64> = unique.iter().map(|d| (d.day.as_str(), d.count)).collect();

    let mut present: Vec<&str> = total_map.keys().chain(unique_map.keys()).copied().collect();
    present.sort_unstable();
    present.dedup();
    if present.is_empty() {
        return TimeSeries {
            empty: true,
            ..Default::default()
        };
    }

    let axis: Vec<String> = match (
        NaiveDate::parse_from_str(present[0], "%Y-%m-%d"),
        NaiveDate::parse_from_str(present[present.len() - 1], "%Y-%m-%d"),
    ) {
        (Ok(first), Ok(last)) => {
            let mut days = Vec::new();
            let mut cursor = first;
            loop {
                days.push(cursor.format("%Y-%m-%d").to_string());
                if cursor >= last {
                    break;
                }
                match cursor.succ_opt() {
                    Some(next) => cursor = next,
                    None => break,
                }
            }
            days
        }
        // Unparseable date (shouldn't happen — substr(ts,1,10) is always YYYY-MM-DD):
        // fall back to the sparse present days, no gap-fill.
        _ => present.iter().map(|s| s.to_string()).collect(),
    };

    let total_series = axis
        .iter()
        .map(|d| *total_map.get(d.as_str()).unwrap_or(&0))
        .collect();
    let unique_series = axis
        .iter()
        .map(|d| *unique_map.get(d.as_str()).unwrap_or(&0))
        .collect();

    TimeSeries {
        days: axis,
        total: total_series,
        unique: unique_series,
        empty: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dc(day: &str, count: i64) -> DayCount {
        DayCount {
            day: day.to_string(),
            count,
        }
    }

    #[test]
    fn timeseries_zero_fills_interior_days() {
        // total has a gap on the 2nd; unique only on the 1st + 3rd. The axis must be a
        // continuous 3-day run with interior no-traffic days as literal 0.
        let total = vec![dc("2026-06-01", 10), dc("2026-06-03", 30)];
        let unique = vec![dc("2026-06-01", 4), dc("2026-06-03", 9)];
        let ts = shape_timeseries(&total, &unique);
        assert!(!ts.empty);
        assert_eq!(ts.days, vec!["2026-06-01", "2026-06-02", "2026-06-03"]);
        assert_eq!(ts.total, vec![10, 0, 30], "the 2nd zero-fills");
        assert_eq!(ts.unique, vec![4, 0, 9]);
        assert_eq!(ts.total.len(), ts.unique.len(), "series stay aligned");
    }

    #[test]
    fn timeseries_empty_is_flagged() {
        let ts = shape_timeseries(&[], &[]);
        assert!(ts.empty);
        assert!(ts.days.is_empty());
    }

    #[test]
    fn timeseries_aligns_when_only_one_series_has_a_day() {
        // unique has a day total doesn't → axis still spans it, total zero-fills there.
        let total = vec![dc("2026-06-01", 5)];
        let unique = vec![dc("2026-06-01", 2), dc("2026-06-02", 1)];
        let ts = shape_timeseries(&total, &unique);
        assert_eq!(ts.days, vec!["2026-06-01", "2026-06-02"]);
        assert_eq!(ts.total, vec![5, 0]);
        assert_eq!(ts.unique, vec![2, 1]);
    }
}
