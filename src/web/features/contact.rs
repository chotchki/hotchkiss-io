use askama::Template;
use axum::response::IntoResponse;

use crate::web::html_template::HtmlTemplate;

use super::navigation_setting::NavSetting;

#[derive(Template)]
#[template(path = "contact.html")]
pub struct ContactTemplate {
    nav: NavSetting,
}

pub async fn contact() -> impl IntoResponse {
    let template = ContactTemplate {
        nav: NavSetting::Contact,
    };

    HtmlTemplate(template)
}
