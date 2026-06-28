use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http::header::AUTHORIZATION;

use crate::db::dao::{api_keys::ApiKeyDao, users::UserDao};
use crate::web::{
    app_state::AppState, authentication_state::AuthenticationState, session::SessionData,
};

/// API-key authentication (Phase CA). Resolves an `Authorization: Bearer hio_…`
/// key and, if it's a live key, injects an Authenticated `SessionData` for the
/// key's user into the request. The `SessionData` extractor reads that injection
/// first, so the existing fail-closed authz layer sees the key's user — DELEGATING
/// that user's role with no cookie + zero handler changes. A missing / non-`hio_`
/// / unknown / revoked key injects nothing, leaving the request on the normal
/// cookie-session path (→ Anonymous → a mutation 403s).
///
/// Wired with `from_fn_with_state` (it needs the pool) and layered OUTER to the
/// authz + session layers so the injection is present when `SessionData` is read.
pub async fn api_key_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    // Pull the token out as an owned String before any await — don't hold a header
    // borrow across the DB lookup or the extensions insert.
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|t| t.starts_with("hio_"))
        .map(str::to_string);

    if let Some(token) = token {
        if let Some(session_data) = resolve(&state, &token).await {
            req.extensions_mut().insert(session_data);
        }
    }

    next.run(req).await
}

/// Look up a live key → its user → an Authenticated `SessionData`. Errors are
/// swallowed to `None` (fail-closed: a broken/unknown key auths as nobody, never
/// 500s the request).
async fn resolve(state: &AppState, token: &str) -> Option<SessionData> {
    let (user_id, key_id) = ApiKeyDao::authenticate(&state.pool, token).await.ok()??;
    let user = UserDao::find_by_uuid(&state.pool, &user_id).await.ok()??;
    if let Err(e) = ApiKeyDao::touch_last_used(&state.pool, key_id).await {
        tracing::warn!("api-key last_used stamp failed (non-fatal): {e}");
    }
    Some(SessionData {
        auth_state: AuthenticationState::Authenticated(user),
    })
}
