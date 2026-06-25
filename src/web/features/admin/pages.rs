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
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Template)]
#[template(path = "admin/pages.html")]
pub struct AdminPagesTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub pages: Vec<ContentPageDao>,
}

/// Dedicated page-management view (admin-only via the `/admin` require_admin
/// layer): lists the top-level pages with view/edit links, a create-by-title
/// form, and drag-to-reorder. Replaces the in-nav inline create — the nav's `+`
/// links here.
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

/// Submitted by the SortableJS+htmx drag-to-reorder: the top-level page ids in
/// their new visual order. `serde_html_form` (axum-extra `Form`) collects the
/// repeated `page_id` keys into a Vec.
#[derive(Deserialize)]
pub struct ReorderForm {
    #[serde(default)]
    pub page_id: Vec<i64>,
}

/// Persist a new top-level page order: `page_order` becomes the page's index in
/// the submitted list. Admin-gated (the /admin require_admin layer + the
/// fail-closed non-GET layer). Returns 200 with no body — the client drag
/// already reflects the new order (hx-swap="none").
pub async fn reorder_pages(
    State(state): State<AppState>,
    Form(form): Form<ReorderForm>,
) -> Result<Response, AppError> {
    let mut tx = state.pool.begin().await?;

    // Scope the write to the level this endpoint manages: reject any submitted id
    // that isn't a current top-level page, so a crafted POST can't renumber a
    // child/special page's order and corrupt a different list. (Returning before
    // commit rolls the transaction back — no partial write.)
    let top_level: HashSet<i64> = ContentPageDao::find_by_parent(&mut *tx, None)
        .await?
        .into_iter()
        .map(|p| p.page_id)
        .collect();

    for (index, page_id) in form.page_id.iter().enumerate() {
        if !top_level.contains(page_id) {
            return Ok((StatusCode::BAD_REQUEST, "unknown top-level page").into_response());
        }
        ContentPageDao::set_order(&mut *tx, *page_id, index as i64).await?;
    }
    tx.commit().await?;

    Ok(StatusCode::OK.into_response())
}
