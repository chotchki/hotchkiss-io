use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate, htmx_responses::htmx_refresh,
        session::SessionData, util::category,
    },
};
use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::types::chrono::{DateTime, Utc};
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
        top_bar: TopBar::create(&state.pool, "admin", session_data.auth_state.role()).await?,
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

/// The child-index widget's drag-to-reorder (Phase DV.12): the child ids in their
/// new visual order, plus the parent they belong to and the page-order `start`
/// offset (the widget is paginated, so within-page drag writes `start..start+N`,
/// leaving other pages' order untouched).
#[derive(Deserialize)]
pub struct ReorderChildrenForm {
    pub parent_id: i64,
    #[serde(default)]
    pub start: i64,
    #[serde(default)]
    pub page_id: Vec<i64>,
}

/// Persist a new order for a page's CHILDREN (the ` ```children ` widget). Like
/// `reorder_pages` but scoped to one parent + offset-aware for pagination. Admin-
/// gated (the /admin layer). Every submitted id MUST be a child of `parent_id` — a
/// crafted POST can't renumber an unrelated list (rolls back before commit).
pub async fn reorder_children(
    State(state): State<AppState>,
    Form(form): Form<ReorderChildrenForm>,
) -> Result<Response, AppError> {
    let mut tx = state.pool.begin().await?;
    let children: HashSet<i64> = ContentPageDao::find_by_parent(&mut *tx, Some(form.parent_id))
        .await?
        .into_iter()
        .map(|p| p.page_id)
        .collect();

    for (index, page_id) in form.page_id.iter().enumerate() {
        if !children.contains(page_id) {
            return Ok((StatusCode::BAD_REQUEST, "not a child of that page").into_response());
        }
        ContentPageDao::set_order(&mut *tx, *page_id, form.start + index as i64).await?;
    }
    tx.commit().await?;

    Ok(StatusCode::OK.into_response())
}

/// Toggle a page's landing "Featured" pin (Phase 13.8): flip the reserved
/// `featured` tag in its `page_category`. Read-modify-write against the CURRENT DB
/// value (not a form field), so it composes with a page's other category tags and
/// never depends on unsaved editor state. Admin-gated (the `/admin` require_admin
/// layer + the fail-closed non-GET layer); the editor's Pin button posts here by
/// `page_id`, which sidesteps the `/pages/{*path}` catch-all. `htmx_refresh` so the
/// button + the category field re-render with the new state.
pub async fn toggle_feature(
    State(state): State<AppState>,
    Path(page_id): Path<i64>,
) -> Result<Response, AppError> {
    let Some(page) = ContentPageDao::find_by_id(&state.pool, page_id).await? else {
        return Ok((StatusCode::NOT_FOUND, "No such page").into_response());
    };
    let toggled = category::toggle_featured(page.page_category.as_deref());
    ContentPageDao::set_category(&state.pool, page_id, toggled).await?;
    Ok(htmx_refresh())
}

/// Publish a scheduled/draft page NOW (Phase CU): stamp `page_creation_date` to the
/// current instant so it goes live immediately. Read-modify-write by `page_id` (like
/// the Pin button), so it never touches unsaved editor state; `set_creation_date`
/// stamps `page_modified_date` too, keeping the feed/sitemap validators fresh.
/// Admin-gated by the `/admin` require_admin layer.
pub async fn publish_now(
    State(state): State<AppState>,
    Path(page_id): Path<i64>,
) -> Result<Response, AppError> {
    if ContentPageDao::find_by_id(&state.pool, page_id)
        .await?
        .is_none()
    {
        return Ok((StatusCode::NOT_FOUND, "No such page").into_response());
    }
    ContentPageDao::set_creation_date(&state.pool, page_id, Utc::now()).await?;
    Ok(htmx_refresh())
}

/// Unpublish a live page back to a DRAFT (Phase CU): stamp `page_creation_date` far
/// in the future so `is_scheduled()` is true and every public read path hides it. A
/// sentinel, not a real schedule — to schedule for a SPECIFIC time set the editor's
/// Posted field instead; Publish-now later stamps `now`. Admin-gated.
pub async fn unpublish(
    State(state): State<AppState>,
    Path(page_id): Path<i64>,
) -> Result<Response, AppError> {
    if ContentPageDao::find_by_id(&state.pool, page_id)
        .await?
        .is_none()
    {
        return Ok((StatusCode::NOT_FOUND, "No such page").into_response());
    }
    let draft_sentinel: DateTime<Utc> = "2999-01-01T00:00:00Z"
        .parse()
        .expect("valid far-future draft sentinel");
    ContentPageDao::set_creation_date(&state.pool, page_id, draft_sentinel).await?;
    Ok(htmx_refresh())
}
