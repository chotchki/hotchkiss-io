/// Built off this tutorial: https://joeymckenzie.tech/blog/templates-with-rust-axum-htmx-askama
use coordinator::service_coordinator::ServiceCoordinator;
use rustls::crypto::ring;
use serde::{Deserialize, Serialize};
use std::{env, fs};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod coordinator;
mod web;

#[derive(Serialize, Deserialize)]
struct Settings {
    pub cloudflare_token: String,
    pub database_path: String,
    pub domain: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ring::default_provider()
        .install_default()
        .expect("Crypto provider ring unable to be installed");

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hotchkiss_io=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Hotchkiss IO Starting Up");

    //Read our sensitive data from stdin
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        error!(
            "You must pass the config file path as an argument, got {:?}",
            args
        );
        return Ok(());
    }

    let config = fs::read_to_string(args.get(1).unwrap())?;
    let settings: Settings = serde_json::from_str(&config)?;

    //Build the coordinator
    let mut coordinator = ServiceCoordinator::create(settings).await?;

    info!("Starting up the coordinator");
    coordinator.start().await?; //This never returns

    Ok(())
}
