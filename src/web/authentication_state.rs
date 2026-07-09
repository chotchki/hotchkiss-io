use crate::db::dao::{roles::Role, users::UserDao};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use webauthn_rs::prelude::{DiscoverableAuthentication, PasskeyRegistration};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub enum AuthenticationState {
    #[default]
    Anonymous,
    AuthOptions(DiscoverableAuthentication),
    RegistrationStarted((PasskeyRegistration, UserDao)),
    Authenticated(UserDao),
}

impl AuthenticationState {
    pub fn is_admin(&self) -> bool {
        self.role() == Role::Admin
    }

    /// The viewer's role — `Anonymous` unless fully `Authenticated` (the
    /// ceremony states carry a user but haven't proven possession yet). Every
    /// viewer-role derivation (visibility gates, the role-scoped mutation
    /// allowlist) goes through this, never an inline match.
    pub fn role(&self) -> Role {
        match self {
            AuthenticationState::Authenticated(user) => user.role,
            _ => Role::Anonymous,
        }
    }

    /// Whether ANY user is logged in (session or API key) — used by the greylist toll to wave
    /// through an authenticated human of any role, not just an admin.
    pub fn is_authenticated(&self) -> bool {
        matches!(self, AuthenticationState::Authenticated(_))
    }

    /// The authenticated user, if any — for handlers that need the current user
    /// (e.g. scoping API keys to their owner).
    pub fn user(&self) -> Option<&UserDao> {
        match self {
            AuthenticationState::Authenticated(user) => Some(user),
            _ => None,
        }
    }

    /// The logged-in user's display name, if authenticated — drives the nav's
    /// login-state indicator (`Some` ⇒ show the name + a logout link).
    pub fn display_name(&self) -> Option<&str> {
        match self {
            AuthenticationState::Authenticated(user) => Some(&user.display_name),
            _ => None,
        }
    }
}

impl Display for AuthenticationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthenticationState::Anonymous => write!(f, "AuthState: Anonymous"),
            AuthenticationState::AuthOptions(_) => write!(f, "AuthState: AuthOptions"),
            AuthenticationState::RegistrationStarted((_, u)) => {
                write!(f, "AuthState: RegistrationStarted for {u}")
            }
            AuthenticationState::Authenticated(u) => {
                write!(f, "AuthState: Authenticated for {u}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn admin_check() {
        assert!(!AuthenticationState::Anonymous.is_admin());

        let registered = UserDao {
            display_name: "registered".to_string(),
            id: Uuid::new_v4(),
            keys: sqlx::types::Json(vec![]),
            role: Role::Registered,
        };
        assert!(!AuthenticationState::Authenticated(registered).is_admin());

        let admin = UserDao {
            display_name: "admin".to_string(),
            id: Uuid::new_v4(),
            keys: sqlx::types::Json(vec![]),
            role: Role::Admin,
        };
        assert!(AuthenticationState::Authenticated(admin).is_admin());
    }

    /// `role()` surfaces the authenticated user's real role and collapses every
    /// non-authenticated state to `Anonymous`.
    #[test]
    fn role_derivation() {
        assert_eq!(AuthenticationState::Anonymous.role(), Role::Anonymous);

        let user = |role| UserDao {
            display_name: format!("user-{role}"),
            id: Uuid::new_v4(),
            keys: sqlx::types::Json(vec![]),
            role,
        };
        for role in [Role::Registered, Role::Family, Role::Admin] {
            assert_eq!(AuthenticationState::Authenticated(user(role)).role(), role);
        }
        // Family is trusted but NOT admin.
        assert!(!AuthenticationState::Authenticated(user(Role::Family)).is_admin());
    }
}
