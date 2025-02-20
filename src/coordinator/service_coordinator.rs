use std::sync::Arc;

use super::acme_provider_service::AcmeProviderService;
use super::dns::cloudflare_client::CloudflareClient;
use super::dns::dns_validator::DnsValidator;
use super::dns_provider_service::DnsProviderService;
use super::endpoints_provider_service::EndpointsProviderService;
use crate::{
    coordinator::ip_provider_service::IpProviderService, db::database_handle::DatabaseHandle,
    Settings,
};
/// The goal of the coordinator is to start up the various dependancies of the server AND
/// be able to reconfigure it automatically at runtime.
use anyhow::{Context, Result};
use hickory_resolver::TokioAsyncResolver;
use tokio::sync::broadcast;
use tracing::error;

pub struct ServiceCoordinator {
    ip_provider_service: IpProviderService,
    dns_provider_service: DnsProviderService,
    acme_provider_service: AcmeProviderService,
    endpoints_provider_service: EndpointsProviderService,
}

impl ServiceCoordinator {
    pub async fn create(settings: Arc<Settings>) -> Result<Self> {
        let pool = DatabaseHandle::create(&settings.database_path).await?;
        let resolver = TokioAsyncResolver::tokio_from_system_conf()?;
        let dns_validator = DnsValidator::new(resolver.clone());
        let cloudflare_client = CloudflareClient::new(settings.clone(), dns_validator.clone())?;

        let ip_provider_service = IpProviderService::create()?;
        let dns_provider_service = DnsProviderService::create(
            settings.clone(),
            resolver.clone(),
            cloudflare_client.clone(),
        );
        let acme_provider_service =
            AcmeProviderService::create(settings.clone(), pool.clone(), cloudflare_client)?;
        let endpoints_provider_service =
            EndpointsProviderService::create(settings.clone(), pool.clone()).await?;

        Ok(Self {
            ip_provider_service,
            dns_provider_service,
            acme_provider_service,
            endpoints_provider_service,
        })
    }

    pub async fn start(self) -> Result<()> {
        let (ip_provider_sender, ip_provider_reciever) = broadcast::channel(1);
        let (tls_config_sender, tls_config_reciever) = broadcast::channel(1);
        let (endpoint_started_sender, _) = broadcast::channel(1);

        let ips = self.ip_provider_service;
        let dps = self.dns_provider_service;
        let aps = self.acme_provider_service;
        let eps = self.endpoints_provider_service;

        let _ = tokio::try_join!(
            tokio::spawn(async move { ips.start(ip_provider_sender).await }),
            tokio::spawn(async move { dps.start(ip_provider_reciever).await }),
            tokio::spawn(async move { aps.start(tls_config_sender).await }),
            tokio::spawn(async move {
                eps.start(tls_config_reciever, endpoint_started_sender)
                    .await
            })
        )
        .context("A subservice failed")?;

        error!("This should only appear in the case of failure");

        Ok(())
    }
}
