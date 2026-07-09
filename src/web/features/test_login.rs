//! Debug-only test-login seam: `POST /test/login[?role=Admin|Registered|Family]` mints
//! a fresh user with that role and puts an `Authenticated` session on the
//! request, so integration tests (and local poking) can reach role-gated routes
//! without the WebAuthn dance. `#[cfg(debug_assertions)]` — the route and this
//! whole module are absent from `--release` builds (= prod).

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use sqlx::query;
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    db::dao::{roles::Role, users::UserDao},
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        session::SessionData,
    },
};

pub fn test_router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login_as))
        // Exercises the CatchPanicLayer: a handler panic must surface as a styled
        // 500, NOT a dropped connection.
        .route("/panic", get(trigger_panic))
}

/// Always panics — for the CatchPanicLayer integration test only.
async fn trigger_panic() -> Response {
    panic!("intentional test panic (CatchPanicLayer)")
}

#[derive(Deserialize)]
struct TestLoginQuery {
    role: Option<Role>,
}

/// `POST /test/login` — no `role` ⇒ `Admin` (the useful default for poking at
/// admin pages); `?role=Registered` / `?role=Family` for non-admin sessions.
/// (`role` is case-sensitive — the strum variant names, e.g. `Admin`, `Family`.)
/// Always creates a fresh user; a test DB is fresh per `spawn_test_server`.
async fn login_as(
    State(state): State<AppState>,
    session: Session,
    Query(q): Query<TestLoginQuery>,
) -> Result<Response, AppError> {
    let role = q.role.unwrap_or(Role::Admin);
    let id = Uuid::new_v4();
    let display_name = format!("test-{role}");
    let id_str = id.to_string();
    let role_str = role.to_string();

    query!(
        r#"INSERT INTO users (display_name, id, keys, app_role) VALUES (?1, ?2, '[]', ?3)"#,
        display_name,
        id_str,
        role_str,
    )
    .execute(&state.pool)
    .await?;

    let user = UserDao {
        display_name,
        id,
        keys: sqlx::types::Json(Vec::new()),
        role,
    };
    SessionData::update_session(
        &session,
        &SessionData {
            auth_state: AuthenticationState::Authenticated(user),
        },
    )
    .await?;

    Ok((StatusCode::OK, format!("logged in as {role}")).into_response())
}
