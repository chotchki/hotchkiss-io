use crate::db::dao::users::User;
use axum::{
    extract::FromRequestParts,
    http::{self, request::Parts},
};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use webauthn_rs::prelude::{
    DiscoverableAuthentication, PasskeyAuthentication, PasskeyRegistration,
};

#[derive(Clone, Deserialize, Serialize)]
pub enum AuthenticationState {
    Anonymous,
    AuthOptions(DiscoverableAuthentication),
    AuthenticationStarted(PasskeyAuthentication),
    RegistrationStarted((PasskeyRegistration, User)),
    Authenticated(User),
}

#[derive(Clone, Deserialize, Serialize)]
pub struct SessionData {
    pub auth_state: AuthenticationState,
}

impl SessionData {
    const SESSION_DATA_KEY: &str = "session_data";

    pub async fn update_session(session: &Session, session_data: &SessionData) {
        session
            .insert(Self::SESSION_DATA_KEY, session_data.clone())
            .await
            .unwrap()
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
        let session_data: SessionData = session
            .get(Self::SESSION_DATA_KEY)
            .await
            .unwrap() //Unsure how to do this without an unwrap
            .unwrap_or_default();

        Ok(session_data)
    }
}
