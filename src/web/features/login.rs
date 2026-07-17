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
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tower_sessions::Session;
use tracing::{debug, info, warn};
use uuid::Uuid;
use webauthn_rs::prelude::{
    DiscoverableKey, PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse,
};

use super::top_bar::TopBar;

pub fn login_router() -> Router<AppState> {
    Router::new()
        .route("/", get(login_page))
        .route("/js_required", get(js_required_page))
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
    /// Server-rendered message in the login form's error slot. `None` on the
    /// normal page; `Some` on the `/login/js_required` fallback (DM.4), where a
    /// browser that blocked the passkey ceremony (JS off / htmx failed to load)
    /// lands instead of silently reloading.
    error: Option<String>,
}

/// The single hard client requirement the ceremony can't work around: a name.
/// (The passkey step needs a modern browser + JS — that's surfaced by the
/// `js_required` fallback + `<noscript>`, not here.) Validated server-side too
/// so a direct caller — not just the browser — gets a real reason, not a 500
/// (DM.7). Permissive by design: spaces/unicode/punctuation are fine.
fn validate_display_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("Please enter a username.");
    }
    if name.chars().count() > 64 {
        return Err("That username is too long (64 characters max).");
    }
    if name.chars().any(char::is_control) {
        return Err("That username contains invalid characters.");
    }
    Ok(())
}

/// True when an `anyhow`-wrapped DAO error is a SQLite UNIQUE/PK violation —
/// used to turn the raw `users(display_name)` constraint into an actionable 409
/// instead of a leaked-schema 500 (DM.6 backstop). Mirrors the DK.1
/// `is_unique_violation` in `web/features/pages/write.rs`.
fn is_unique_violation(e: &anyhow::Error) -> bool {
    e.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|db| db.is_unique_violation())
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
        error: None,
    };

    Ok(HtmlTemplate(template))
}

/// DM.4 fallback: the login form's native/htmx submit lands here when the
/// passkey ceremony couldn't run (JS disabled, or htmx / htmx-webauthn.js failed
/// to load), instead of a silent reload or a 401 HX-Redirect loop. A public GET,
/// so it never trips the mutation gate.
async fn js_required_page(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<HtmlTemplate<LoginTemplate>, AppError> {
    let template = LoginTemplate {
        top_bar: TopBar::create(&state.pool, "login", session_data.auth_state.role()).await?,
        auth_state: session_data.auth_state,
        error: Some(
            "Your browser blocked the passkey step (JavaScript may be disabled or failed to \
             load). Enable JavaScript and use a recent Safari (16.4+), Chrome, or Firefox."
                .to_string(),
        ),
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
) -> Result<Response, AppError> {
    stash_next(&session, &query).await?;

    // DM.7: the browser encodeURIComponent's the name, but a direct caller can
    // send anything — validate server-side so a bad name is a real 400, not a
    // 404 / silent truncation / 500.
    let display_name = display_name.trim().to_string();
    if let Err(msg) = validate_display_name(&display_name) {
        info!("registration rejected: invalid display_name ({msg})");
        return Ok((StatusCode::BAD_REQUEST, msg).into_response());
    }

    // DM.6: reject a taken name BEFORE the passkey is minted on the device, so a
    // collision can't strand a real credential behind a bare 500.
    if UserDao::find_by_display_name(&state.pool, &display_name)
        .await?
        .is_some()
    {
        info!("registration rejected: display_name {display_name:?} already taken");
        return Ok((
            StatusCode::CONFLICT,
            format!("The name \"{display_name}\" is already taken — pick another."),
        )
            .into_response());
    }

    let user_unique_id = Uuid::new_v4();

    // Initiate a basic registration flow, allowing any cryptographic authenticator to proceed.
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
        display_name: display_name.clone(),
        id: user_unique_id,
        keys: sqlx::types::Json(vec![]),
        role: Role::Anonymous,
    };

    session_data.auth_state = AuthenticationState::RegistrationStarted((passkey_reg, user));
    SessionData::update_session(&session, &session_data).await?;
    info!("registration started for display_name {display_name:?}");
    Ok(Json(ccr).into_response())
}

async fn finish_registration(
    State(state): State<AppState>,
    session: Session,
    mut session_data: SessionData,
    Json(rpc): Json<RegisterPublicKeyCredential>,
) -> Result<Response, AppError> {
    if let AuthenticationState::RegistrationStarted((rs, mut user)) = session_data.auth_state {
        let passkey = state.webauthn.finish_passkey_registration(&rpc, &rs)?;

        if UserDao::find_by_passkey(&state.pool, &passkey)
            .await?
            .is_some()
        {
            warn!(
                "registration blocked: passkey already registered (attempted name {:?})",
                user.display_name
            );
            return Ok((
                StatusCode::CONFLICT,
                "That passkey is already registered on this device — try logging in instead.",
            )
                .into_response());
        };

        user.keys = sqlx::types::Json(vec![passkey]);
        user.role = Role::Registered;

        // DM.6 backstop: a name taken between the pre-check and here (a TOCTOU
        // race) hits the UNIQUE(display_name) PK. Turn it into a real 409, not a
        // bare 500 that strands the just-minted passkey with no reason.
        match user.create(&state.pool).await {
            Ok(()) => {}
            Err(e) if is_unique_violation(&e) => {
                warn!(
                    "registration blocked: display_name {:?} taken at commit (raced the pre-check)",
                    user.display_name
                );
                return Ok((
                    StatusCode::CONFLICT,
                    format!(
                        "The name \"{}\" is already taken — pick another.",
                        user.display_name
                    ),
                )
                    .into_response());
            }
            Err(e) => return Err(e.into()),
        }

        session.cycle_id().await?;
        info!(
            "registration succeeded: new {} user {:?} ({})",
            user.role, user.display_name, user.id
        );
        session_data.auth_state = AuthenticationState::Authenticated(user);

        SessionData::update_session(&session, &session_data).await?;

        Ok(Redirect::to(&pop_next(&session).await).into_response())
    } else {
        warn!("finish_register called with no registration in progress");
        Ok((
            StatusCode::BAD_REQUEST,
            "Registration wasn't started, or your session expired — please start again.",
        )
            .into_response())
    }
}

async fn logout(session: Session, mut session_data: SessionData) -> Result<Redirect, AppError> {
    debug!("Logging out {:#?}", session_data.auth_state);
    session_data.auth_state = AuthenticationState::Anonymous;
    SessionData::update_session(&session, &session_data).await?;
    Ok(Redirect::to("/"))
}
