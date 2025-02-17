use crate::web::app_state::AppState;
use axum::Router;
use content::content_router;
use management::management_router;

pub mod attachments;
pub mod content;
pub mod management;
pub mod projects;

pub fn pages_router() -> Router<AppState> {
    content_router().merge(management_router())
}
