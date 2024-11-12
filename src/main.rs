use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Building from here: https://joeymckenzie.tech/blog/templates-with-rust-axum-htmx-askama
#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hotchkiss_io=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("hello, web server!");
}
