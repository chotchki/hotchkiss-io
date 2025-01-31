use std::fmt::Display;

use crate::db::dao::users::User;
use anyhow::Result;
use axum::{
    extract::FromRequestParts,
    http::{self, request::Parts},
};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::debug;
use webauthn_rs::prelude::{DiscoverableAuthentication, PasskeyRegistration};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum AuthenticationState {
    Anonymous,
    AuthOptions(DiscoverableAuthentication),
    RegistrationStarted((PasskeyRegistration, User)),
    Authenticated(User),
}

impl Display for AuthenticationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthenticationState::Anonymous => write!(f, "AuthState: Anonymous"),
            AuthenticationState::AuthOptions(_) => write!(f, "AuthState: AuthOptions"),
            AuthenticationState::RegistrationStarted((_, u)) => {
                write!(f, "AuthState: RegistrationStarted for {}", u)
            }
            AuthenticationState::Authenticated(u) => {
                write!(f, "AuthState: Authenticated for {}", u)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionData {
    pub auth_state: AuthenticationState,
}

impl SessionData {
    const SESSION_DATA_KEY: &str = "session_data";

    pub async fn update_session(session: &Session, session_data: &SessionData) -> Result<()> {
        Ok(session
            .insert(Self::SESSION_DATA_KEY, session_data.clone())
            .await?)
    }
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            auth_state: AuthenticationState::Anonymous,
        }
    }
}

impl<S> FromRequestParts<S> for SessionData
where
    S: Send + Sync,
{
    type Rejection = (http::StatusCode, &'static str);

    async fn from_request_parts(req: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(req, state).await?;

        debug!("Found session! {:?}", session.id());

        let session_data: SessionData = session
            .get(Self::SESSION_DATA_KEY)
            .await
            .unwrap() //Unsure how to do this without an unwrap
            .unwrap_or_default();

        debug!("Session auth state {}", session_data.auth_state);

        Ok(session_data)
    }
}
