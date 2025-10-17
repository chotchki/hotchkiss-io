use std::sync::Arc;

use super::acme_provider_service::AcmeProviderService;
use super::dns::cloudflare_client::CloudflareClient;
use super::dns::dns_validator::DnsValidator;
use super::dns_provider_service::DnsProviderService;
use super::endpoints_provider_service::EndpointsProviderService;
use crate::settings::Settings;
use crate::{
    coordinator::ip_provider_service::IpProviderService, db::database_handle::DatabaseHandle,
};
use anyhow::{Context, Result, bail};
use hickory_resolver::TokioAsyncResolver;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::error;

/// The goal of the coordinator is to start up the various dependancies of the server AND
/// be able to reconfigure it automatically at runtime.
pub struct ServiceCoordinator {
    ip_provider_service: IpProviderService,
    dns_provider_service: DnsProviderService,
    acme_provider_service: AcmeProviderService,
    endpoints_provider_service: EndpointsProviderService,
}

impl ServiceCoordinator {
    pub async fn create(settings: Settings) -> Result<Self> {
        let settings = Arc::new(settings);
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

        let ips = self.ip_provider_service;
        let dps = self.dns_provider_service;
        let aps = self.acme_provider_service;
        let eps = self.endpoints_provider_service;

        let ips_handle = tokio::spawn(async move { ips.start(ip_provider_sender).await });
        let dps_handle = tokio::spawn(async move { dps.start(ip_provider_reciever).await });
        let aps_handle = tokio::spawn(async move { aps.start(tls_config_sender).await });
        let eps_handle = tokio::spawn(async move { eps.start(tls_config_reciever).await });

        async fn flatten<T>(handle: JoinHandle<Result<T>>) -> Result<T> {
            match handle.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(err)) => Err(err),
                Err(err) => bail!("handling failed {}", err),
            }
        }

        match tokio::try_join!(
            flatten(ips_handle),
            flatten(dps_handle),
            flatten(aps_handle),
            flatten(eps_handle)
        ) {
            Ok(_) => unreachable!("This should only appear in the case of failure"),
            Err(e) => {
                error!("A service failed {}", e);
                bail!("A service failed {}", e);
            }
        }
    }
}
