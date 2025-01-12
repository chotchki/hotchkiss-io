use askama::Template;
use axum::response::IntoResponse;

use crate::web::html_template::HtmlTemplate;

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

pub async fn authenticationOptions() {}
