use super::acme_provider_service::AcmeProviderService;
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

pub struct ServiceCoordinator {
    ip_provider_service: IpProviderService,
    dns_provider_service: DnsProviderService,
    acme_provider_service: AcmeProviderService,
    endpoints_provider_service: EndpointsProviderService,
}

impl ServiceCoordinator {
    pub async fn create(settings: Settings) -> Result<Self> {
        let pool = DatabaseHandle::create(&settings.database_path).await?;
        let resolver = TokioAsyncResolver::tokio_from_system_conf()?;

        let ip_provider_service = IpProviderService::create()?;
        let dns_provider_service = DnsProviderService::create(
            resolver.clone(),
            settings.cloudflare_token.clone(),
            settings.domain.clone(),
        )?;
        let acme_provider_service = AcmeProviderService::create(
            pool.clone(),
            resolver.clone(),
            settings.cloudflare_token.clone(),
            settings.domain.clone(),
        )?;
        let endpoints_provider_service =
            EndpointsProviderService::create(settings, pool.clone()).await?;

        Ok(Self {
            //installation_status_service,
            ip_provider_service,
            //dns_server_service,
            //install_endpoints,
            dns_provider_service,
            acme_provider_service,
            endpoints_provider_service,
        })
    }

    pub async fn start(self) -> Result<()> {
        let (ip_provider_sender, ip_provider_reciever) = broadcast::channel(1);
        let (tls_config_sender, tls_config_reciever) = broadcast::channel(1);

        let ips = self.ip_provider_service;
        let dps = self.dns_provider_service;
        let aps = self.acme_provider_service;
        let eps = self.endpoints_provider_service;
        let _ = tokio::try_join!(
            tokio::spawn(async move { ips.start(ip_provider_sender).await }),
            tokio::spawn(async move { dps.start(ip_provider_reciever).await }),
            tokio::spawn(async move { aps.start(tls_config_sender).await }),
            tokio::spawn(async move { eps.start(tls_config_reciever).await })
        )
        .context("A subservice failed")?;

        Ok(())
    }
}
