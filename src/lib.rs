use std::{env, io, sync::Arc};

use rustls::crypto::ring;
use tracing::{Level, info};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{Layer, filter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
use tray_wrapper::{ContinueRunning, ServerGeneratorResult, create_tray_wrapper};

use crate::web::router::BUILD_TIME_CACHE_BUST;
use crate::{coordinator::service_coordinator::ServiceCoordinator, settings::Settings};
mod coordinator;
mod db;
mod settings;
mod web;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn global_init() {
    ring::default_provider()
        .install_default()
        .expect("Crypto provider ring unable to be installed");
}

fn create_server() -> ServerGeneratorResult {
    let settings = match Settings::load(env::args()) {
        Ok(s) => s,
        Err(e) => {
            return Box::pin(async move {
                println!("No settings {}", e);
                ContinueRunning::ExitWithError(format!("No settings {}", e))
            });
        }
    };

    let app_filter = filter::Targets::new()
        .with_target("hotchkiss_io", Level::DEBUG)
        .with_target("hotchkiss_io::web::static_content", Level::DEBUG)
        .with_target("axum::rejection", Level::TRACE)
        .with_default(Level::INFO);
    let access_filter = filter::Targets::new().with_target("tower_http", Level::DEBUG);

    let app_rolling =
        RollingFileAppender::new(Rotation::DAILY, &settings.log_path, "hotchkiss.io.log");
    let access_rolling =
        RollingFileAppender::new(Rotation::DAILY, &settings.log_path, "access.log");

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .compact()
                .with_ansi(true)
                .with_writer(io::stdout)
                .with_filter(app_filter.clone()),
        )
        .with(
            fmt::layer()
                .compact()
                .with_ansi(false)
                .with_writer(app_rolling)
                .with_filter(app_filter),
        )
        .with(
            fmt::layer()
                .compact()
                .with_ansi(false)
                .with_writer(access_rolling)
                .with_filter(access_filter),
        )
        .init();

    info!("Hotchkiss IO Starting Up: version {VERSION} frontend timestamp {BUILD_TIME_CACHE_BUST}");

    let task = async move {
        //Build the coordinator
        let Ok(coordinator) = ServiceCoordinator::create(settings).await else {
            return ContinueRunning::ExitWithError(
                "Unable to create the service coordinator".to_string(),
            );
        };

        info!("Starting up the coordinator");
        let Ok(_) = coordinator.start().await else {
            return ContinueRunning::Continue;
        };
        ContinueRunning::Continue
    };
    Box::pin(task)
}

pub fn real_main() -> anyhow::Result<()> {
    global_init();

    create_tray_wrapper(
        include_bytes!("../assets/images/HotchkissLogox1024.png"),
        Some(VERSION.to_string()),
        Arc::new(&create_server),
    )?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_global_init() {
        global_init()
    }
}
