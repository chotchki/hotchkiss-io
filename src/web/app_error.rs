use axum::response::{IntoResponse, Response};
use http::StatusCode;
use tracing::error;
use uuid::Uuid;

// Example used to wrap our errors sanely: https://github.com/tokio-rs/axum/blob/main/examples/anyhow-error-response/src/main.rs

// Make our own error that wraps `anyhow::Error`.
pub struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let id = Uuid::new_v4();
        error!("Error trace id: {} for {:#}", id.to_string(), self.0);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong, trace id: {}", id),
        )
            .into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
