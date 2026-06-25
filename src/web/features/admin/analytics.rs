use askama::Template;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::{
    db::dao::request_log::{DayCount, PathCount, RefererCount, RequestLogDao, UserAgentCount},
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate, session::SessionData,
    },
};

#[derive(Deserialize)]
pub struct AnalyticsQuery {
    pub since: Option<i64>,
    pub metric: Option<String>,
    pub paths: Option<String>,
}

#[derive(Template)]
#[template(path = "analytics/dashboard.html")]
pub struct AnalyticsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub since_days: i64,
    /// "total" (page views) or "unique" (distinct-IP visitors) — drives the
    /// chart series + which toggle chip is active.
    pub metric: String,
    /// "content" (status < 400) or "all" (incl. 4xx/5xx scanner probes) — drives
    /// the Top Pages list + which toggle chip is active.
    pub paths_mode: String,
    pub total_requests: i64,
    pub distinct_ips: i64,
    /// Server-rendered inline SVG bar chart of the selected metric per day.
    pub chart_svg: String,
    pub by_path: Vec<PathCount>,
    pub by_user_agent: Vec<UserAgentCount>,
    /// Top external referrers — directional only (spoofable / often stripped).
    pub by_referer: Vec<RefererCount>,
    pub recent: Vec<RequestLogDao>,
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
    let metric = if q.metric.as_deref() == Some("unique") {
        "unique"
    } else {
        "total"
    };
    // Top-pages filter: "content" (successful loads only) hides the 404 scanner
    // probes; "all" surfaces them (status ceiling raised) so chris can see what's
    // attacking the site. Static assets are excluded either way.
    let paths_mode = if q.paths.as_deref() == Some("all") {
        "all"
    } else {
        "content"
    };
    let max_status = if paths_mode == "all" { 10_000 } else { 400 };

    let total_requests = RequestLogDao::count_since(&state.pool, since_days).await?;
    let distinct_ips = RequestLogDao::distinct_ip_count(&state.pool, since_days).await?;
    let by_day = if metric == "unique" {
        RequestLogDao::distinct_ip_by_day(&state.pool, since_days).await?
    } else {
        RequestLogDao::count_by_day(&state.pool, since_days).await?
    };
    let by_path = RequestLogDao::count_by_content_path(&state.pool, since_days, max_status, 25).await?;
    let by_user_agent = RequestLogDao::count_by_user_agent(&state.pool, since_days, 25).await?;
    let by_referer = RequestLogDao::count_by_referer(&state.pool, since_days, 25).await?;
    let recent = RequestLogDao::recent(&state.pool, 50).await?;

    let tmpl = AnalyticsTemplate {
        top_bar: TopBar::create(&state.pool, "").await?,
        auth_state: session_data.auth_state,
        since_days,
        metric: metric.to_string(),
        paths_mode: paths_mode.to_string(),
        total_requests,
        distinct_ips,
        chart_svg: bar_chart_svg(&by_day),
        by_path,
        by_user_agent,
        by_referer,
        recent,
    };
    Ok(HtmlTemplate(tmpl).into_response())
}

/// Render a per-day bar chart as a self-contained, responsive inline SVG (no
/// client-side chart library). Bars scale to the window's max; each carries a
/// `<title>` for hover. Empty windows render a "no data" note rather than a gap.
fn bar_chart_svg(days: &[DayCount]) -> String {
    const W: f64 = 760.0;
    const H: f64 = 220.0;
    const PAD_L: f64 = 40.0;
    const PAD_R: f64 = 10.0;
    const PAD_T: f64 = 12.0;
    const PAD_B: f64 = 26.0;

    let open = format!(
        "<svg viewBox=\"0 0 {W} {H}\" preserveAspectRatio=\"xMidYMid meet\" \
role=\"img\" aria-label=\"per-day chart\" style=\"width:100%;height:auto;max-width:{W}px;font-family:sans-serif\">"
    );

    if days.is_empty() {
        return format!(
            "{open}<text x=\"{:.0}\" y=\"{:.0}\" font-size=\"13\" fill=\"#6b7280\" text-anchor=\"middle\">No views in this range.</text></svg>",
            W / 2.0,
            H / 2.0,
        );
    }

    let plot_w = W - PAD_L - PAD_R;
    let plot_h = H - PAD_T - PAD_B;
    let baseline_y = PAD_T + plot_h;
    let max = days.iter().map(|d| d.count).max().unwrap_or(0).max(1);
    let slot = plot_w / days.len() as f64;
    let bar_w = (slot * 0.72).clamp(1.0, 48.0);

    let mut bars = String::new();
    for (i, d) in days.iter().enumerate() {
        let bh = plot_h * (d.count as f64 / max as f64);
        let x = PAD_L + i as f64 * slot + (slot - bar_w) / 2.0;
        let y = baseline_y - bh;
        bars.push_str(&format!(
            "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{bar_w:.1}\" height=\"{bh:.1}\" fill=\"#14213d\"><title>{}: {}</title></rect>",
            d.day, d.count
        ));
    }

    let baseline_x2 = W - PAD_R;
    let maxlabel_y = PAD_T + 10.0;
    let first = days.first().map(|d| d.day.as_str()).unwrap_or("");
    let last = days.last().map(|d| d.day.as_str()).unwrap_or("");
    let dates_y = H - 8.0;

    format!(
        "{open}\
<line x1=\"{PAD_L:.1}\" y1=\"{baseline_y:.1}\" x2=\"{baseline_x2:.1}\" y2=\"{baseline_y:.1}\" stroke=\"#9ca3af\" stroke-width=\"1\"/>\
<text x=\"{:.1}\" y=\"{maxlabel_y:.1}\" font-size=\"11\" fill=\"#14213d\" text-anchor=\"end\">{max}</text>\
<text x=\"{:.1}\" y=\"{baseline_y:.1}\" font-size=\"11\" fill=\"#14213d\" text-anchor=\"end\">0</text>\
<text x=\"{PAD_L:.1}\" y=\"{dates_y:.1}\" font-size=\"11\" fill=\"#6b7280\">{first}</text>\
<text x=\"{baseline_x2:.1}\" y=\"{dates_y:.1}\" font-size=\"11\" fill=\"#6b7280\" text-anchor=\"end\">{last}</text>\
{bars}</svg>",
        PAD_L - 5.0,
        PAD_L - 5.0,
    )
}
