use anyhow::Result;
use axum::{
    extract::FromRequestParts,
    http::{self, request::Parts},
};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::debug;

use super::authentication_state::AuthenticationState;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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

impl<S> FromRequestParts<S> for SessionData
where
    S: Send + Sync,
{
    type Rejection = (http::StatusCode, &'static str);

    async fn from_request_parts(req: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(req, state).await?;

        debug!("Found session! {:?}", session.id());

        // A session-store read error (e.g. transient SQLITE_BUSY, or a stale
        // deserialize) must NOT panic the request — fall back to Anonymous
        // (fail-closed) and log it. The session layer is INNER to the authz
        // layer, so an Anonymous fallback simply 403s a mutation, never 500s.
        let session_data: SessionData = match session.get(Self::SESSION_DATA_KEY).await {
            Ok(opt) => opt.unwrap_or_default(),
            Err(e) => {
                tracing::warn!("session read failed, treating as anonymous: {e}");
                SessionData::default()
            }
        };

        debug!("Session auth state {}", session_data.auth_state);

        Ok(session_data)
    }
}
