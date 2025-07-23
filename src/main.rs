/// Built off this tutorial: https://joeymckenzie.tech/blog/templates-with-rust-axum-htmx-askama
use anyhow::Context;
use coordinator::service_coordinator::ServiceCoordinator;
use rustls::crypto::ring;
use std::{env, fs, io, sync::Arc};
use tracing::{Level, info};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{Layer, filter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

mod coordinator;
pub mod db;
mod settings;
use settings::Settings;
mod web;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ring::default_provider()
        .install_default()
        .expect("Crypto provider ring unable to be installed");

    let args: Vec<String> = env::args().skip(1).take(1).collect();

    let config = if args.is_empty()
        && let Some(home) = env::home_dir()
    {
        let mut app_dir_default = home;
        app_dir_default.push("Library");
        app_dir_default.push("Application Support");
        app_dir_default.push("io.hotchkiss.web");
        fs::DirBuilder::new()
            .recursive(true)
            .create(&app_dir_default)?;

        let mut config_path = app_dir_default;
        config_path.push("config.json");

        fs::read_to_string(&config_path)
            .with_context(|| format!("No config path passed, could not open {config_path:?}"))?
    } else {
        fs::read_to_string(
            args.first()
                .with_context(|| format!("First argument must be the config file, got {args:?}"))?,
        )?
    };

    let settings: Arc<Settings> = Arc::new(serde_json::from_str(&config).with_context(|| {
        format!("Failed to parse settings file to settings struct content:{config}")
    })?);

    let app_filter = filter::Targets::new()
        .with_target("hotchkiss_io", Level::DEBUG)
        .with_target("hotchkiss_io::web::static_content", Level::INFO)
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

    info!("Hotchkiss IO Starting Up");

    //Build the coordinator
    let coordinator = ServiceCoordinator::create(settings).await?;

    info!("Starting up the coordinator");
    coordinator.start().await?; //This never returns

    Ok(())
}
