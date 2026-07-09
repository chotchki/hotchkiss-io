use crate::web::app_error::AppError;
use crate::web::authentication_state::AuthenticationState;
use crate::{
    db::dao::{roles::Role, users::UserDao},
    web::{app_state::AppState, html_template::HtmlTemplate, session::SessionData},
};
use anyhow::{anyhow, Context};
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::Redirect,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tower_sessions::Session;
use tracing::debug;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, DiscoverableKey, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

use super::top_bar::TopBar;

pub fn login_router() -> Router<AppState> {
    Router::new()
        .route("/", get(login_page))
        .route("/get_auth_opts", get(authentication_options))
        .route("/finish_authentication", post(finish_authentication))
        .route("/start_register/{:display_name}", get(start_registration))
        .route("/finish_register", post(finish_registration))
        .route("/logout", get(logout))
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    top_bar: TopBar,
    auth_state: AuthenticationState,
}

/// Session key for the validated post-login destination (Phase DE). The
/// ceremony is `fetch()`-driven — a hidden form field has nowhere to ride — so
/// the destination is stashed server-side alongside the ceremony state and
/// popped by the finish handlers. One-shot; last-stashed wins.
const LOGIN_NEXT_KEY: &str = "login_next";

/// `?next=` on the login GET routes — optional, strictly validated.
#[derive(Deserialize, Default)]
struct NextQuery {
    next: Option<String>,
}

/// Stash a VALID `?next` into the session; invalid/absent values never write
/// (an open-redirect string must not survive to the pop, and an absent param
/// must not clobber a stash from an earlier hop of the same flow).
async fn stash_next(session: &Session, query: &NextQuery) -> Result<(), AppError> {
    if let Some(next) = query.next.as_deref()
        && let Some(valid) = crate::web::util::next_url::safe_next(next)
    {
        session.insert(LOGIN_NEXT_KEY, valid.to_string()).await?;
    }
    Ok(())
}

/// Pop the stashed destination, RE-validating on the way out (the session
/// value is attacker-adjacent state; validation rules may also have tightened
/// since it was written). Fallback: `/`.
async fn pop_next(session: &Session) -> String {
    let stashed: Option<String> = session.remove(LOGIN_NEXT_KEY).await.ok().flatten();
    stashed
        .as_deref()
        .and_then(crate::web::util::next_url::safe_next)
        .unwrap_or("/")
        .to_string()
}

async fn login_page(
    State(state): State<AppState>,
    session: Session,
    session_data: SessionData,
    Query(query): Query<NextQuery>,
) -> Result<HtmlTemplate<LoginTemplate>, AppError> {
    stash_next(&session, &query).await?;
    let template = LoginTemplate {
        top_bar: TopBar::create(&state.pool, "login", session_data.auth_state.role()).await?,
        auth_state: session_data.auth_state,
    };

    Ok(HtmlTemplate(template))
}

async fn authentication_options(
    State(state): State<AppState>,
    session: Session,
    mut session_data: SessionData,
    Query(query): Query<NextQuery>,
) -> Result<Json<RequestChallengeResponse>, AppError> {
    if let AuthenticationState::Authenticated(_) = session_data.auth_state {
        return Err(anyhow!("Already logged in").into());
    }
    stash_next(&session, &query).await?;

    let (challenge, discoverable_auth) = state.webauthn.start_discoverable_authentication()?;
    session_data.auth_state = AuthenticationState::AuthOptions(discoverable_auth);
    SessionData::update_session(&session, &session_data).await?;
    Ok(Json(challenge))
}

async fn finish_authentication(
    State(state): State<AppState>,
    session: Session,
    mut session_data: SessionData,
    Json(pkc): Json<PublicKeyCredential>,
) -> Result<Redirect, AppError> {
    let (client_uuid, _) = state.webauthn.identify_discoverable_authentication(&pkc)?;

    let mut user = UserDao::find_by_uuid(&state.pool, &client_uuid)
        .await?
        .ok_or(anyhow!("User not found"))?;

    let creds: Vec<DiscoverableKey> = user.keys.iter().map(|x| x.into()).collect();

    if let AuthenticationState::AuthOptions(ao) = session_data.auth_state {
        let auth_result = state
            .webauthn
            .finish_discoverable_authentication(&pkc, ao, &creds)?;
        if auth_result.needs_update() {
            user.keys.iter_mut().for_each(|x| {
                x.update_credential(&auth_result);
            });

            user.update(&state.pool).await?;
        }

        debug!("Logged in {:#?}", user);

        session.cycle_id().await?;
        session_data.auth_state = AuthenticationState::Authenticated(user);
        SessionData::update_session(&session, &session_data).await?;

        // The JS navigates to this response's final URL (fetch follows the
        // redirect), so the stashed ?next lands the user where the gate
        // sent them to log in.
        Ok(Redirect::to(&pop_next(&session).await))
    } else {
        Err(anyhow!("Authentication not in progress").into())
    }
}

async fn start_registration(
    State(state): State<AppState>,
    session: Session,
    Path(display_name): Path<String>,
    mut session_data: SessionData,
    Query(query): Query<NextQuery>,
) -> Result<Json<CreationChallengeResponse>, AppError> {
    stash_next(&session, &query).await?;
    let user_unique_id = Uuid::new_v4();

    // Initiate a basic registration flow, allowing any cryptograhpic authenticator to proceed.
    let (ccr, passkey_reg) = state
        .webauthn
        .start_passkey_registration(
            user_unique_id,
            &display_name,
            &display_name,
            None, // No other credentials are registered yet.
        )
        .context("Failed to start registration.")?;

    let user = UserDao {
        display_name,
        id: user_unique_id,
        keys: sqlx::types::Json(vec![]),
        role: Role::Anonymous,
    };

    session_data.auth_state = AuthenticationState::RegistrationStarted((passkey_reg, user));
    SessionData::update_session(&session, &session_data).await?;
    Ok(Json(ccr))
}

async fn finish_registration(
    State(state): State<AppState>,
    session: Session,
    mut session_data: SessionData,
    Json(rpc): Json<RegisterPublicKeyCredential>,
) -> Result<Redirect, AppError> {
    if let AuthenticationState::RegistrationStarted((rs, mut user)) = session_data.auth_state {
        let passkey = state.webauthn.finish_passkey_registration(&rpc, &rs)?;

        if UserDao::find_by_passkey(&state.pool, &passkey)
            .await?
            .is_some()
        {
            return Err(anyhow!("Passkey already registered").into());
        };

        user.keys = sqlx::types::Json(vec![passkey]);
        user.role = Role::Registered;

        user.create(&state.pool).await?;

        session.cycle_id().await?;
        session_data.auth_state = AuthenticationState::Authenticated(user);

        SessionData::update_session(&session, &session_data).await?;

        Ok(Redirect::to(&pop_next(&session).await))
    } else {
        Err(anyhow!("Registration not in progress").into())
    }
}

async fn logout(session: Session, mut session_data: SessionData) -> Result<Redirect, AppError> {
    debug!("Logging out {:#?}", session_data.auth_state);
    session_data.auth_state = AuthenticationState::Anonymous;
    SessionData::update_session(&session, &session_data).await?;
    Ok(Redirect::to("/"))
}
