use axum::{
    extract::FromRequestParts,
    http::{self, request::Parts},
};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use webauthn_rs::prelude::PasskeyAuthentication;

pub const SESSION_KEY: &str = "session";

#[derive(Default, Clone, Deserialize, Serialize)]
struct SessionData {
    pub auth_challenge: Option<PasskeyAuthentication>,
}

impl<S> FromRequestParts<S> for SessionData
where
    S: Send + Sync,
{
    type Rejection = (http::StatusCode, &'static str);

    async fn from_request_parts(req: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(req, state).await?;
        let session_data: SessionData = session.get(SESSION_KEY).await.unwrap().unwrap_or_default();

        Ok(session_data)
    }
}
