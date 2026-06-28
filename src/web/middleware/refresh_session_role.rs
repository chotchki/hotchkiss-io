use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use tower_sessions::Session;

use crate::db::dao::users::UserDao;
use crate::web::{
    app_state::AppState, authentication_state::AuthenticationState, session::SessionData,
};

/// Live role enforcement (Phase CC). A cookie session stores a SNAPSHOT of the
/// authenticated user — role included — so a role change or delete wouldn't bite a
/// live session until logout / the 1-day expiry. This middleware re-loads the
/// cookie session's user from the DB on each request and injects a refreshed
/// `SessionData` (current role), or Anonymous if the user was deleted — so a
/// demote/delete takes effect IMMEDIATELY. The `SessionData` extractor reads that
/// injection first.
///
/// Layered INNER to `api_key_auth`: if a `Bearer` key already injected an identity
/// (which it resolves fresh anyway), this is a no-op. Uses the same inject-into-
/// extensions trick because the `SessionData` extractor is generic and can't reach
/// the DB pool.
pub async fn refresh_session_role(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    // api_key_auth (outer) already resolved a fresh identity → leave it.
    if req.extensions().get::<SessionData>().is_some() {
        return next.run(req).await;
    }

    // The session layer (outer) put a `Session` in extensions. Only an
    // Authenticated cookie session needs refreshing — the WebAuthn ceremony states
    // (AuthOptions / RegistrationStarted) stay on the normal cookie path so the
    // login handlers read them intact.
    if let Some(session) = req.extensions().get::<Session>().cloned()
        && let Ok(Some(data)) = session.get::<SessionData>(SessionData::SESSION_DATA_KEY).await
        && let AuthenticationState::Authenticated(user) = &data.auth_state
    {
        match UserDao::find_by_uuid(&state.pool, &user.id).await {
            Ok(Some(fresh)) => {
                // Refresh the role (and display name) from the DB.
                req.extensions_mut().insert(SessionData {
                    auth_state: AuthenticationState::Authenticated(fresh),
                });
            }
            Ok(None) => {
                // Deleted → downgrade this request to Anonymous.
                req.extensions_mut().insert(SessionData::default());
            }
            Err(e) => {
                // Transient DB error: don't lock out the legit user over a blip —
                // leave the request on the cookie session (the extractor reads it).
                tracing::warn!("session role refresh failed, using cookie role: {e}");
            }
        }
    }

    next.run(req).await
}
