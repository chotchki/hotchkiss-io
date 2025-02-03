use axum::Router;
use content::content_router;
use management::management_router;

use crate::web::app_state::AppState;

pub mod content;
pub mod management;

pub fn pages_router() -> Router<AppState> {
    content_router().merge(management_router())
}
