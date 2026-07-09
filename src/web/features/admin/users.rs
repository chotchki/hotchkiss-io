//! User management (Phase CC, three-way since CZ): list users and move them
//! between Registered / Family / Admin, or delete. Admin-gated by the `/admin`
//! nest's `require_admin` layer. The last admin is protected from demote/delete
//! (no lockout); role + delete take effect immediately on a live session via
//! the `refresh_session_role` middleware. `Anonymous` is a sentinel, never an
//! assignable role — the handler rejects it.

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

/// Every role an admin may assign — `Anonymous` is deliberately absent (it's
/// the not-logged-in sentinel, not an account level).
const ASSIGNABLE_ROLES: [Role; 3] = [Role::Registered, Role::Family, Role::Admin];

/// One rendered row of the user list.
pub struct UserRow {
    pub display_name: String,
    pub id: String,
    /// The user's REAL role — rendered as the badge (`Display` = variant name).
    pub role: Role,
    /// Roles this row can be moved to: the assignable set minus the current
    /// role; empty for the last admin (their role is locked, no lockout).
    pub role_targets: Vec<Role>,
    pub passkey_count: i64,
    pub api_key_count: i64,
    /// This row is the currently logged-in admin (label "(you)").
    pub is_self: bool,
    /// Admin AND the only admin — protected from demote/delete (no lockout); the
    /// UI hides those actions and the handlers reject them.
    pub is_last_admin: bool,
}

impl UserRow {
    /// Badge styling: Admin inverse navy-on-yellow, Family yellow (trusted
    /// household tier), Registered muted.
    fn badge_class(&self) -> &'static str {
        match self.role {
            Role::Admin => "bg-navy text-yellow",
            Role::Family => "bg-yellow text-navy",
            _ => "bg-navy/10 text-navy",
        }
    }
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
            let is_last_admin = s.role == Role::Admin && admin_count <= 1;
            UserRow {
                is_self: Some(s.id) == me,
                is_last_admin,
                display_name: s.display_name,
                id: s.id.to_string(),
                role_targets: if is_last_admin {
                    vec![]
                } else {
                    ASSIGNABLE_ROLES.into_iter().filter(|r| *r != s.role).collect()
                },
                role: s.role,
                passkey_count: s.passkey_count,
                api_key_count: s.api_key_count,
            }
        })
        .collect();

    let tmpl = UsersTemplate {
        top_bar: TopBar::create(&state.pool, "admin", session_data.auth_state.role()).await?,
        auth_state: session_data.auth_state,
        users,
    };
    Ok(HtmlTemplate(tmpl).into_response())
}

#[derive(Deserialize)]
pub struct RoleForm {
    pub role: Role,
}

/// `POST /admin/users/{id}/role` — move a user between Registered / Family /
/// Admin. Rejects `Anonymous` as a target (a sentinel, not an account level)
/// and demoting the final admin (the UI already hides those actions; these are
/// the defense-in-depth guards).
pub async fn set_user_role(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Form(form): Form<RoleForm>,
) -> Result<Response, AppError> {
    // Positive allowlist, not a `!= Anonymous` check: a future sentinel variant
    // must not slip through just because nobody added it to a deny-list.
    if !ASSIGNABLE_ROLES.contains(&form.role) {
        return Ok((StatusCode::BAD_REQUEST, "Not an assignable role").into_response());
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    /// Forces a deliberate assignability decision for every future `Role`
    /// variant (mirrors the `rank()` ladder pin in roles.rs): today's rule is
    /// "every variant except the `Anonymous` sentinel". A new variant fails
    /// here until it's added to `ASSIGNABLE_ROLES` or this rule is consciously
    /// amended — without this, a new role would silently be unassignable in
    /// the UI while the handler's validation drifted separately.
    #[test]
    fn assignable_roles_cover_every_non_sentinel_variant() {
        let mut expected: Vec<Role> = Role::iter().filter(|r| *r != Role::Anonymous).collect();
        expected.sort_by_key(|r| r.rank());
        let mut actual = ASSIGNABLE_ROLES.to_vec();
        actual.sort_by_key(|r| r.rank());
        assert_eq!(actual, expected);
    }
}
