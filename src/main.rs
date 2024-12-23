/// Built off this tutorial: https://joeymckenzie.tech/blog/templates-with-rust-axum-htmx-askama
use anyhow::Context;
use coordinator::service_coordinator::ServiceCoordinator;
use rustls::crypto::ring;
use std::{env, fs, io};
use tracing::{info, Level};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{filter, fmt, layer::SubscriberExt, util::SubscriberInitExt, Layer};

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

    let config = fs::read_to_string(
        args.first()
            .with_context(|| format!("First argument must be the config file, got {args:?}"))?,
    )?;
    let settings: Settings = serde_json::from_str(&config).with_context(|| {
        format!("Failed to parse settings file to settings struct content:{config}")
    })?;

    let app_filter = filter::Targets::new().with_target("hotchkiss_io", Level::DEBUG);
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
    let mut coordinator = ServiceCoordinator::create(settings).await?;

    info!("Starting up the coordinator");
    coordinator.start().await?; //This never returns

    Ok(())
}
