use askama::Template;
use axum::{extract::State, response::IntoResponse, Json};
use tower_sessions::Session;
use webauthn_rs::prelude::RequestChallengeResponse;

use crate::web::{
    app_state::AppState, html_template::HtmlTemplate, router::AppError, session::SESSION_KEY,
};

use super::navigation_setting::NavSetting;

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    nav: NavSetting,
}

pub async fn loginPage() -> impl IntoResponse {
    let template = LoginTemplate {
        nav: NavSetting::Login,
    };

    HtmlTemplate(template)
}

pub async fn authenticationOptions(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<RequestChallengeResponse>, AppError> {
    let (challenge, passkeyAuth) = state.webauthn.start_passkey_authentication(&[])?;
    session.insert(SESSION_KEY, passkeyAuth).await?;
    Ok(Json(challenge))
}
