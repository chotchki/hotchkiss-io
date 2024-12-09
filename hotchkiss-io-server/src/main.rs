/// Built off this tutorial: https://joeymckenzie.tech/blog/templates-with-rust-axum-htmx-askama
use coordinator::{ip::omada_config::OmadaConfig, service_coordinator::ServiceCoordinator};
use rustls::crypto::ring;
use serde::{Deserialize, Serialize};
use std::io;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod coordinator;
mod web;

#[derive(Serialize, Deserialize)]
struct Settings {
    pub cloudflare_token: String,
    pub database_path: String,
    pub domain: String,
    pub omada_config: OmadaConfig,
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

    //Read our sensitive data from stdin
    let stdin = io::read_to_string(io::stdin())?;
    let settings: Settings = serde_json::from_str(&stdin)?;

    //Build the coordinator
    let mut coordinator = ServiceCoordinator::create(settings).await?;

    info!("Starting up the coordinator");
    coordinator.start().await?; //This never returns

    Ok(())
}
