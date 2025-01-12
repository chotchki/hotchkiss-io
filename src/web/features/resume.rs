use askama::Template;
use axum::response::IntoResponse;

use crate::web::html_template::HtmlTemplate;

use super::navigation_setting::NavSetting;

#[derive(Template)]
#[template(path = "resume.html")]
pub struct ResumeTemplate {
    nav: NavSetting,
}

pub async fn resume() -> impl IntoResponse {
    let template = ResumeTemplate {
        nav: NavSetting::Resume,
    };
    HtmlTemplate(template)
}
