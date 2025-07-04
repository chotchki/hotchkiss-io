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
        match self {
            AuthenticationState::Authenticated(user) => user.role == Role::Admin,
            _ => false,
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
}
