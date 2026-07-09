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
/// Since Phase CZ this middleware is ALSO the session TOUCH: it re-saves the
/// refreshed session on every authenticated request, which is what makes
/// `Expiry::OnInactivity(1 day)` actually mean inactivity (tower-sessions only
/// pushes the expiry forward on a session WRITE, and login used to be the only
/// write). Anonymous traffic carries no authenticated session → no write.
///
/// Layered INNER to `api_key_auth`: if a `Bearer` key already injected an identity
/// (which it resolves fresh anyway), this is a no-op. Uses the same inject-into-
/// extensions trick because the `SessionData` extractor is generic and can't reach
/// the DB pool.
/// Session key holding the unix-seconds of the last expiry touch.
const TOUCHED_AT_KEY: &str = "touched_at";
/// Touch at most hourly — see the THROTTLED comment below.
const SESSION_TOUCH_INTERVAL_SECS: i64 = 3600;

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
                let refreshed = SessionData {
                    auth_state: AuthenticationState::Authenticated(fresh),
                };
                // Keep the stored snapshot current — free when nothing changed
                // (`insert` dedups identical values), a real write when the
                // role/name did change.
                if let Err(e) = SessionData::update_session(&session, &refreshed).await {
                    tracing::warn!("session snapshot refresh failed: {e}");
                }
                // Session TOUCH (Phase CZ): tower-sessions only pushes the
                // `OnInactivity(1 day)` expiry forward when the session is
                // WRITTEN, and before this the app wrote it exactly once (at
                // login) — every session died 24h after login no matter how
                // active the user was. The snapshot insert above does NOT
                // guarantee the write: `insert` skips marking the session
                // modified when the value is unchanged (the common case).
                // Stamping `touched_at` with a NEW timestamp marks it modified
                // under any insert semantics, and the layer's save recomputes
                // `expiry_date` = now + 1 day.
                //
                // THROTTLED to one write per hour per session: this middleware
                // wraps static assets and `/media` too, so an authenticated
                // browser's asset fan-out (or an audiobook's range-request
                // storm, Phase DD) would otherwise turn every subresource GET
                // into a `tower_sessions` write — and a save that trips the 5s
                // busy_timeout becomes a 500 (tower-sessions REPLACES the
                // response on save failure). Worst case a session expires up
                // to 1h of activity-credit early; irrelevant on a 24h window.
                //
                // Accepted residual race: a request in flight during logout can
                // last-write-wins the record back to Authenticated (logout
                // overwrites with Anonymous, it doesn't flush). Inherent to any
                // activity-refresh session scheme; the throttle bounds it to
                // ~one racing write per session-hour, and a deleted/demoted
                // user gains nothing — this middleware re-derives the role from
                // the DB on every request.
                let now = time::OffsetDateTime::now_utc().unix_timestamp();
                let touched_at: Option<i64> =
                    session.get(TOUCHED_AT_KEY).await.unwrap_or_default();
                if touched_at.is_none_or(|t| now - t >= SESSION_TOUCH_INTERVAL_SECS) {
                    if let Err(e) = session.insert(TOUCHED_AT_KEY, now).await {
                        tracing::warn!("session touch failed (expiry not extended): {e}");
                    }
                }
                // Refresh the role (and display name) from the DB.
                req.extensions_mut().insert(refreshed);
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
