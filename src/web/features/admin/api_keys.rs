use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use http::StatusCode;
use serde::Deserialize;

use crate::db::dao::api_keys::ApiKeyDao;
use crate::web::features::top_bar::TopBar;
use crate::web::htmx_responses::htmx_refresh;
use crate::web::{
    app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
    html_template::HtmlTemplate, session::SessionData,
};

#[derive(Template)]
#[template(path = "admin/api_keys.html")]
pub struct ApiKeysTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub keys: Vec<ApiKeyView>,
    /// The plaintext key — set ONLY on the response right after creation, shown
    /// exactly once (never stored, never recoverable).
    pub new_key: Option<String>,
}

/// A key for display — never the hash or plaintext.
pub struct ApiKeyView {
    pub id: i64,
    pub label: String,
    pub created: String,
    pub last_used: String,
    pub revoked: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateKeyForm {
    pub label: String,
}

pub async fn show_api_keys(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    render_page(&state, session_data, None).await
}

pub async fn create_api_key(
    State(state): State<AppState>,
    session_data: SessionData,
    Form(form): Form<CreateKeyForm>,
) -> Result<Response, AppError> {
    let Some(user_id) = session_data.auth_state.user().map(|u| u.id) else {
        return Ok((StatusCode::FORBIDDEN, "Not authenticated").into_response());
    };
    let label = form.label.trim();
    if label.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "A label is required").into_response());
    }
    let (key, _) = ApiKeyDao::create(&state.pool, &user_id, label).await?;
    // Re-render the page carrying the plaintext — the ONE time it's shown.
    render_page(&state, session_data, Some(key)).await
}

pub async fn revoke_api_key(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let Some(user_id) = session_data.auth_state.user().map(|u| u.id) else {
        return Ok((StatusCode::FORBIDDEN, "Not authenticated").into_response());
    };
    // Scoped to the user inside the DAO, so you can only revoke your own.
    ApiKeyDao::revoke(&state.pool, id, &user_id).await?;
    Ok(htmx_refresh())
}

async fn render_page(
    state: &AppState,
    session_data: SessionData,
    new_key: Option<String>,
) -> Result<Response, AppError> {
    let Some(user_id) = session_data.auth_state.user().map(|u| u.id) else {
        return Ok((StatusCode::FORBIDDEN, "Not authenticated").into_response());
    };
    let keys = ApiKeyDao::list_for_user(&state.pool, &user_id)
        .await?
        .into_iter()
        .map(|k| ApiKeyView {
            id: k.id,
            label: k.label,
            created: k.created_at.format("%Y-%m-%d").to_string(),
            last_used: k
                .last_used_at
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "never".to_string()),
            revoked: k.revoked_at.map(|t| t.format("%Y-%m-%d").to_string()),
        })
        .collect();

    let template = ApiKeysTemplate {
        top_bar: TopBar::create(&state.pool, "admin").await?,
        auth_state: session_data.auth_state,
        keys,
        new_key,
    };
    Ok(HtmlTemplate(template).into_response())
}
