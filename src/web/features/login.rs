use crate::web::app_error::AppError;
use crate::{
    db::dao::{roles::Role, users::UserDao},
    web::{
        app_state::AppState,
        html_template::HtmlTemplate,
        session::{AuthenticationState, SessionData},
    },
};
use anyhow::{anyhow, Context};
use askama::Template;
use axum::{
    extract::{Path, State},
    response::Redirect,
    routing::{get, post},
    Json, Router,
};
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

async fn login_page(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<HtmlTemplate<LoginTemplate>, AppError> {
    let template = LoginTemplate {
        top_bar: TopBar::create(&state.pool, "login").await?,
        auth_state: session_data.auth_state,
    };

    Ok(HtmlTemplate(template))
}

async fn authentication_options(
    State(state): State<AppState>,
    session: Session,
    mut session_data: SessionData,
) -> Result<Json<RequestChallengeResponse>, AppError> {
    if let AuthenticationState::Authenticated(_) = session_data.auth_state {
        return Err(anyhow!("Already logged in").into());
    }

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

        Ok(Redirect::to("/"))
    } else {
        Err(anyhow!("Authentication not in progress").into())
    }
}

async fn start_registration(
    State(state): State<AppState>,
    session: Session,
    Path(display_name): Path<String>,
    mut session_data: SessionData,
) -> Result<Json<CreationChallengeResponse>, AppError> {
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

        Ok(Redirect::to("/"))
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
