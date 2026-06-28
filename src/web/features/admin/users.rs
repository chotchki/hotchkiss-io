//! User management (Phase CC): list users, promote/demote between
//! Registered↔Admin, and delete. Admin-gated by the `/admin` nest's
//! `require_admin` layer. The last admin is protected from demote/delete (no
//! lockout); role + delete take effect immediately on a live session via the
//! `refresh_session_role` middleware.

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    db::dao::{roles::Role, users::UserDao},
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::top_bar::TopBar, html_template::HtmlTemplate, htmx_responses::htmx_refresh,
        session::SessionData,
    },
};

/// One rendered row of the user list.
pub struct UserRow {
    pub display_name: String,
    pub id: String,
    pub is_admin: bool,
    pub passkey_count: i64,
    pub api_key_count: i64,
    /// This row is the currently logged-in admin (label "(you)").
    pub is_self: bool,
    /// Admin AND the only admin — protected from demote/delete (no lockout); the
    /// UI hides those actions and the handlers reject them.
    pub is_last_admin: bool,
}

#[derive(Template)]
#[template(path = "admin/users.html")]
pub struct UsersTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub users: Vec<UserRow>,
}

pub async fn show_users(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let admin_count = UserDao::count_admins(&state.pool).await?;
    let me = session_data.auth_state.user().map(|u| u.id);

    let users = UserDao::list_summaries(&state.pool)
        .await?
        .into_iter()
        .map(|s| {
            let is_admin = s.role == Role::Admin;
            UserRow {
                is_self: Some(s.id) == me,
                is_last_admin: is_admin && admin_count <= 1,
                display_name: s.display_name,
                id: s.id.to_string(),
                is_admin,
                passkey_count: s.passkey_count,
                api_key_count: s.api_key_count,
            }
        })
        .collect();

    let tmpl = UsersTemplate {
        top_bar: TopBar::create(&state.pool, "admin").await?,
        auth_state: session_data.auth_state,
        users,
    };
    Ok(HtmlTemplate(tmpl).into_response())
}

#[derive(Deserialize)]
pub struct RoleForm {
    pub role: Role,
}

/// `POST /admin/users/{id}/role` — promote/demote. Rejects demoting the final
/// admin (the UI already hides that action; this is the defense-in-depth guard).
pub async fn set_user_role(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Form(form): Form<RoleForm>,
) -> Result<Response, AppError> {
    let Some(target) = UserDao::find_by_uuid(&state.pool, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "No such user").into_response());
    };

    let demoting_an_admin = target.role == Role::Admin && form.role != Role::Admin;
    if demoting_an_admin && UserDao::count_admins(&state.pool).await? <= 1 {
        return Ok((StatusCode::CONFLICT, "Can't demote the last admin").into_response());
    }

    UserDao::set_role(&state.pool, &id, form.role).await?;
    Ok(htmx_refresh())
}

/// `DELETE /admin/users/{id}` — delete a user (cascades their API keys).
/// Rejects deleting the final admin.
pub async fn delete_user(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let Some(target) = UserDao::find_by_uuid(&state.pool, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "No such user").into_response());
    };

    if target.role == Role::Admin && UserDao::count_admins(&state.pool).await? <= 1 {
        return Ok((StatusCode::CONFLICT, "Can't delete the last admin").into_response());
    }

    UserDao::delete(&state.pool, &id).await?;
    Ok(htmx_refresh())
}
