use anyhow::Context;
use askama::Template;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use tower_sessions::Session;
use uuid::Uuid;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse};

use crate::web::{
    app_state::AppState, html_template::HtmlTemplate, router::AppError, session::SESSION_KEY,
};

use super::navigation_setting::NavSetting;

pub const START_AUTH: &str = "start_auth";
pub const START_REGISTRATION: &str = "start_registration";

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    nav: NavSetting,
}

pub async fn login_page() -> impl IntoResponse {
    let template = LoginTemplate {
        nav: NavSetting::Login,
    };

    HtmlTemplate(template)
}

pub async fn authentication_options(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<RequestChallengeResponse>, AppError> {
    let (challenge, passkey_auth) = state.webauthn.start_passkey_authentication(&[])?;
    session.insert(START_AUTH, passkey_auth).await?;
    Ok(Json(challenge))
}

pub async fn start_registration(
    State(state): State<AppState>,
    session: Session,
    Path(display_name): Path<String>,
) -> Result<Json<CreationChallengeResponse>, AppError> {
    let user_unique_id = Uuid::new_v4();

    // Initiate a basic registration flow, allowing any cryptograhpic authenticator to proceed.
    let (ccr, skr) = state
        .webauthn
        .start_passkey_registration(
            user_unique_id,
            &display_name,
            &display_name,
            None, // No other credentials are registered yet.
        )
        .context("Failed to start registration.")?;

    session.insert(START_REGISTRATION, skr).await?;

    Ok(Json(ccr))
}
