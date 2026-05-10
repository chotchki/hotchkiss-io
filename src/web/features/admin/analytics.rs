use askama::Template;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::{
    db::dao::request_log::{DayCount, PathCount, RequestLogDao, UserAgentCount},
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate, session::SessionData,
    },
};

#[derive(Deserialize)]
pub struct AnalyticsQuery {
    pub since: Option<i64>,
}

#[derive(Template)]
#[template(path = "analytics/dashboard.html")]
pub struct AnalyticsTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub since_days: i64,
    pub total_requests: i64,
    pub distinct_ips: i64,
    pub by_day: Vec<DayCount>,
    pub by_path: Vec<PathCount>,
    pub by_user_agent: Vec<UserAgentCount>,
    pub recent: Vec<RequestLogDao>,
}

/// `GET /admin/analytics` — gated by the `require_admin` layer on the `admin`
/// router, so no auth check here. `?since=<days>` overrides the 7-day default.
pub async fn show_analytics(
    State(state): State<AppState>,
    session_data: SessionData,
    Query(q): Query<AnalyticsQuery>,
) -> Result<Response, AppError> {
    let since_days = q.since.unwrap_or(7).clamp(1, 365);

    let total_requests = RequestLogDao::count_since(&state.pool, since_days).await?;
    let distinct_ips = RequestLogDao::distinct_ip_count(&state.pool, since_days).await?;
    let by_day = RequestLogDao::count_by_day(&state.pool, since_days).await?;
    let by_path = RequestLogDao::count_by_path(&state.pool, since_days, 25).await?;
    let by_user_agent = RequestLogDao::count_by_user_agent(&state.pool, since_days, 25).await?;
    let recent = RequestLogDao::recent(&state.pool, 50).await?;

    let tmpl = AnalyticsTemplate {
        top_bar: TopBar::create(&state.pool, "").await?,
        auth_state: session_data.auth_state,
        since_days,
        total_requests,
        distinct_ips,
        by_day,
        by_path,
        by_user_agent,
        recent,
    };
    Ok(HtmlTemplate(tmpl).into_response())
}
