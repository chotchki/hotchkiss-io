use askama::Template;
use axum::response::IntoResponse;

use crate::web::html_template::HtmlTemplate;

use super::navigation_setting::NavSetting;

#[derive(Template)]
#[template(path = "projects.html")]
pub struct ProjectsTemplate {
    nav: NavSetting,
}

pub async fn projects() -> impl IntoResponse {
    let template = ProjectsTemplate {
        nav: NavSetting::Projects,
    };
    HtmlTemplate(template)
}
