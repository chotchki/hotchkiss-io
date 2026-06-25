use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate, session::SessionData,
    },
};
use askama::Template;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
};

#[derive(Template)]
#[template(path = "admin/pages.html")]
pub struct AdminPagesTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub pages: Vec<ContentPageDao>,
}

/// Dedicated page-management view (admin-only via the `/admin` require_admin
/// layer): lists the top-level pages with view/edit links and a create-by-title
/// form. Replaces the in-nav inline create — the nav's `+` links here.
pub async fn show_admin_pages(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let pages = ContentPageDao::find_by_parent(&state.pool, None).await?;

    let template = AdminPagesTemplate {
        top_bar: TopBar::create(&state.pool, "").await?,
        auth_state: session_data.auth_state,
        pages,
    };
    Ok(HtmlTemplate(template).into_response())
}
