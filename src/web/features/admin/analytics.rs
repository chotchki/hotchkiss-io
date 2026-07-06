use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

use crate::{
    db::dao::request_log::{
        Audience, AudienceCounts, DayCount, IpPathStatus, NoisyIp, PathCount, RequestLogDao,
        StatusBucketCounts, UserAgentCount, Window, SCAN_DISTINCT_404_THRESHOLD,
    },
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate,
        htmx_responses::htmx_refresh, session::SessionData,
        util::{
            referer::{group_referers, GroupedReferers},
            route::{aggregate_by_route, overall_p95, RouteLatency},
        },
    },
};

use crate::db::dao::greylist::GreylistDao;

#[derive(Deserialize)]
pub struct AnalyticsQuery {
    pub since: Option<i64>,
    pub paths: Option<String>,
    /// "humans" | "bots" | anything-else→All. A String (not a typed enum) so a
    /// bad value degrades to All instead of a deserialize-reject → 500 (CQ.2).
    pub audience: Option<String>,
    /// Custom range bounds (Phase CT), as picker strings interpreted as UTC (the whole
    /// dashboard is UTC). Present + valid → OVERRIDES `since`. Either may be blank:
    /// from-only = "since this instant". A bad/inverted range degrades to the preset.
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Parse a picker value (flatpickr `Y-m-d H:i`, or a native `datetime-local`
/// `Y-m-dTH:i`, optional seconds) as a UTC instant — the dashboard reads UTC end to
/// end, so the picker is UTC too (no tz conversion). Empty/garbage → `None` (that
/// bound is open); never errors.
fn parse_range_dt(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M"]
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok())
        .map(|n| n.and_utc())
}

/// Minimal percent-encoding for a picker value dropped into a toggle link's query
/// string — the value charset is only digits / `-` / `T` or space / `:`, so encoding
/// space + `:` is sufficient (axum decodes them back on the round-trip).
fn encode_qs(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Aligned daily series (CQ.7) for the d3 line chart, serialized into the page as a JSON
/// island. NUMERIC only — `days` are DB `substr(ts,1,10)` date strings, never
/// attacker-controlled — so the island can't carry a `</script>` breakout. `challenged`
/// is the greylist-tolls/day overlay (CY.2), always challenged=1 and independent of the
/// audience filter; the renderer only draws it when it carries a nonzero day.
#[derive(Serialize, Debug, Default)]
pub struct TimeSeries {
    pub days: Vec<String>,
    pub total: Vec<i64>,
    pub unique: Vec<i64>,
    pub challenged: Vec<i64>,
    pub empty: bool,
}

#[derive(Template)]
#[template(path = "analytics/dashboard.html")]
pub struct AnalyticsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub since_days: i64,
    /// Custom-range state (Phase CT): when true, a from/to range is active — no preset
    /// chip highlights, and the picker inputs pre-fill from `from_raw`/`to_raw`.
    pub custom_active: bool,
    pub from_raw: String,
    pub to_raw: String,
    /// The active window as a query fragment (`since=30` OR `from=…&to=…`) every toggle
    /// link carries, so an audience/paths switch keeps the window. Server-built → `|safe`.
    pub window_qs: String,
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
    /// Greylist toll activity over the window (CY.2/CY.8), shown regardless of the selected
    /// audience: tolls served (challenged requests), solves (clearances), the distinct IP counts,
    /// and the IP-based solve rate (`None` until something's been tolled → renders "—").
    pub tolls_served: i64,
    pub clearances: i64,
    pub challenged_ips: i64,
    pub cleared_ips: i64,
    pub solve_rate_pct: Option<i64>,
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

    // Window (Phase CT): a valid custom from/to range OVERRIDES the preset. Either
    // bound may be open (from-only = "since this instant" — the post-deploy p95 case);
    // an inverted from > to is invalid → fall back to the preset (never a 500). All the
    // reads below take this ONE Window, so preset + custom share the exact same path.
    let from_dt = q.from.as_deref().and_then(parse_range_dt);
    let to_dt = q.to.as_deref().and_then(parse_range_dt);
    let valid_range = match (from_dt, to_dt) {
        (Some(f), Some(t)) => f < t,
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => false,
    };
    let (window, custom_active) = if valid_range {
        (Window::custom(from_dt, to_dt), true)
    } else {
        (Window::last_days(since_days), false)
    };
    // Raw picker strings — repopulate the inputs + keep a custom range sticky across the
    // audience/paths toggles. For a PRESET, MIRROR the resolved window into both fields
    // (From = the lower bound, To = now; minute precision to match flatpickr's `Y-m-d
    // H:i`) so clicking 7d/30d/90d makes the picker a WYSIWYG snapshot of the active
    // range that's a concrete starting point to tweak. It's display-only until Apply —
    // the active window stays the live preset (`window_qs` = `since=N`) until then.
    let (from_raw, to_raw) = if custom_active {
        (q.from.clone().unwrap_or_default(), q.to.clone().unwrap_or_default())
    } else {
        let from = window.from.get(..16).unwrap_or(window.from.as_str()).to_string();
        let to = Utc::now().format("%Y-%m-%d %H:%M").to_string();
        (from, to)
    };
    // The window query-string every toggle link carries, so switching audience/paths
    // preserves the active window (preset OR custom). Server-built from a controlled
    // charset → `|safe` in the template.
    let window_qs = if custom_active {
        format!("from={}&to={}", encode_qs(&from_raw), encode_qs(&to_raw))
    } else {
        format!("since={since_days}")
    };

    // Run every independent read CONCURRENTLY (CR.3): WAL + the connection pool (≤10)
    // let these fan out across connections, so the page's wall-clock is ~the slowest
    // query instead of the SUM of ~15 windowed scans (the ~7s → sub-1s win). TopBar
    // joins in (it also reads the pool). `?` on the tuple bubbles the first error.
    let (
        top_bar,
        total_requests,
        distinct_ips,
        audience_counts,
        total_by_day,
        unique_by_day,
        challenged_by_day,
        by_path,
        status_buckets,
        noisy_ips,
        never_succeeded,
        by_user_agent,
        referer_urls,
        direct_count,
        latency_samples,
        slow_requests,
        recent,
        tolls_served,
        challenged_ips,
        clearances,
        cleared_ips,
    ) = tokio::try_join!(
        TopBar::create(&state.pool, "admin"),
        RequestLogDao::count_since(&state.pool, &window, audience),
        RequestLogDao::distinct_ip_count(&state.pool, &window, audience),
        RequestLogDao::audience_counts(&state.pool, &window),
        RequestLogDao::count_by_day(&state.pool, &window, audience),
        RequestLogDao::distinct_ip_by_day(&state.pool, &window, audience),
        // Tolls/day overlay (CY.2) — always challenged=1, INDEPENDENT of the audience filter.
        RequestLogDao::count_by_day(&state.pool, &window, Audience::Challenged),
        RequestLogDao::count_by_content_path(&state.pool, &window, audience, max_status, 25),
        RequestLogDao::count_by_status_bucket(&state.pool, &window, audience),
        RequestLogDao::noisy_ips(&state.pool, &window, 0, 25),
        RequestLogDao::never_succeeded_paths(&state.pool, &window, 25),
        RequestLogDao::count_by_user_agent(&state.pool, &window, 25),
        RequestLogDao::referer_urls_since(&state.pool, &window),
        RequestLogDao::direct_referer_count(&state.pool, &window),
        RequestLogDao::latency_samples(&state.pool, &window),
        RequestLogDao::slowest_requests(&state.pool, &window, 15),
        RequestLogDao::recent(&state.pool, 50),
        // Greylist toll activity (CY.2/CY.8) — window-scoped, INDEPENDENT of the selected
        // audience (like `audience_counts`, these are always-shown sub-metrics).
        RequestLogDao::count_since(&state.pool, &window, Audience::Challenged),
        RequestLogDao::distinct_challenged_ips(&state.pool, &window),
        GreylistDao::count_clearances_since(&state.pool, &window),
        GreylistDao::distinct_cleared_ips_since(&state.pool, &window),
    )?;

    // Derived, Rust-side (cheap): the chart island (both daily series overlaid — the gap
    // is the repeat/scanner signal, `<`-escaped for the XSS boundary), the referer fold,
    // and the latency percentiles (SQLite has no percentile fn).
    let ts_json =
        serde_json::to_string(&shape_timeseries(&total_by_day, &unique_by_day, &challenged_by_day))
            .unwrap_or_else(|_| {
                r#"{"days":[],"total":[],"unique":[],"challenged":[],"empty":true}"#.to_string()
            })
            .replace('<', "\\u003c");
    let referers = group_referers(&referer_urls, &state.site_host, direct_count);
    let has_latency = !latency_samples.is_empty();
    let latency_p95 = overall_p95(&latency_samples);
    let mut slow_routes = aggregate_by_route(&latency_samples);
    slow_routes.truncate(15);

    // Hard-vs-soft (CY.8): of the DISTINCT IPs we walled, how many solved through? `None` until
    // something's actually been tolled (fresh boot / beta-scrubbed) so the view shows "—".
    let solve_rate_pct = (challenged_ips > 0).then(|| cleared_ips * 100 / challenged_ips);

    let tmpl = AnalyticsTemplate {
        top_bar,
        auth_state: session_data.auth_state,
        since_days,
        custom_active,
        from_raw,
        to_raw,
        window_qs,
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
        tolls_served,
        clearances,
        challenged_ips,
        cleared_ips,
        solve_rate_pct,
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
    let window = Window::last_days(since_days);

    let path_status = RequestLogDao::ip_path_status(&state.pool, &ip, &window).await?;
    let user_agents = RequestLogDao::ip_user_agents(&state.pool, &ip, &window).await?;
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
        s429: 0,
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
            429 => sb.s429 += r.count,
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

/// `POST /admin/analytics/reclassify-bots` (CR.2.1) — re-run the `is_bot` classifier
/// over ALL rows (e.g. after retuning the ruleset), then reload the dashboard with the
/// refreshed audience counts. Admin-gated by the `/admin` nest. Awaited: an admin does
/// this deliberately + infrequently, so a few seconds is fine.
pub async fn reclassify_bots(State(state): State<AppState>) -> Result<Response, AppError> {
    RequestLogDao::reclassify_bots(&state.pool, false).await?;
    Ok(htmx_refresh())
}

/// Build the two-series daily time series for the d3 line chart (CQ.7). Zero-fills a
/// CONTINUOUS UTC daily axis between the first and last day that carries traffic —
/// interior no-traffic days become a literal 0 (real data), NOT a null gap. Both
/// series align to the same day axis. Empty input → `empty: true` (the renderer draws
/// a note). If a day string somehow won't parse, falls back to the sparse present-days
/// (no gap-fill) rather than dropping the chart.
fn shape_timeseries(total: &[DayCount], unique: &[DayCount], challenged: &[DayCount]) -> TimeSeries {
    use std::collections::HashMap;

    let total_map: HashMap<&str, i64> = total.iter().map(|d| (d.day.as_str(), d.count)).collect();
    let unique_map: HashMap<&str, i64> = unique.iter().map(|d| (d.day.as_str(), d.count)).collect();
    let challenged_map: HashMap<&str, i64> =
        challenged.iter().map(|d| (d.day.as_str(), d.count)).collect();

    // A day with ONLY tolled traffic (e.g. under an audience=humans filter) still belongs on
    // the axis, so fold challenged days into `present` too — else the toll overlay drops them.
    let mut present: Vec<&str> = total_map
        .keys()
        .chain(unique_map.keys())
        .chain(challenged_map.keys())
        .copied()
        .collect();
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
    let challenged_series = axis
        .iter()
        .map(|d| *challenged_map.get(d.as_str()).unwrap_or(&0))
        .collect();

    TimeSeries {
        days: axis,
        total: total_series,
        unique: unique_series,
        challenged: challenged_series,
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
        // Tolls only on the 2nd — a day with NO total/unique — so the overlay both extends
        // onto the shared axis and zero-fills where there were no tolls (CY.2).
        let challenged = vec![dc("2026-06-02", 7)];
        let ts = shape_timeseries(&total, &unique, &challenged);
        assert!(!ts.empty);
        assert_eq!(ts.days, vec!["2026-06-01", "2026-06-02", "2026-06-03"]);
        assert_eq!(ts.total, vec![10, 0, 30], "the 2nd zero-fills");
        assert_eq!(ts.unique, vec![4, 0, 9]);
        assert_eq!(ts.challenged, vec![0, 7, 0], "the toll overlay aligns to the same axis");
        assert_eq!(ts.total.len(), ts.unique.len(), "series stay aligned");
        assert_eq!(ts.total.len(), ts.challenged.len(), "toll series stays aligned");
    }

    #[test]
    fn timeseries_empty_is_flagged() {
        let ts = shape_timeseries(&[], &[], &[]);
        assert!(ts.empty);
        assert!(ts.days.is_empty());
        assert!(ts.challenged.is_empty());
    }

    #[test]
    fn timeseries_aligns_when_only_one_series_has_a_day() {
        // unique has a day total doesn't → axis still spans it, total zero-fills there.
        let total = vec![dc("2026-06-01", 5)];
        let unique = vec![dc("2026-06-01", 2), dc("2026-06-02", 1)];
        let ts = shape_timeseries(&total, &unique, &[]);
        assert_eq!(ts.days, vec!["2026-06-01", "2026-06-02"]);
        assert_eq!(ts.total, vec![5, 0]);
        assert_eq!(ts.unique, vec![2, 1]);
        assert_eq!(ts.challenged, vec![0, 0], "no tolls → a flat-zero overlay");
    }
}
