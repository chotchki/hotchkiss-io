use std::backtrace::Backtrace;

use super::acme_provider_service::AcmeProviderService;
use super::dns_provider_service::DnsProviderService;
use super::endpoints_provider_service::EndpointsProviderService;
use crate::{
    coordinator::ip_provider_service::IpProviderService, db::database_handle::DatabaseHandle,
    Settings,
};
/// The goal of the coordinator is to start up the various dependancies of the server AND
/// be able to reconfigure it automatically at runtime.
use anyhow::Result;
use hickory_resolver::TokioAsyncResolver;
use sqlx::{Pool, Sqlite};
use tokio::sync::broadcast;

pub struct ServiceCoordinator {
    pool: Pool<Sqlite>,
    resolver: TokioAsyncResolver,
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
            pool,
            resolver,
            //installation_status_service,
            ip_provider_service,
            //dns_server_service,
            //install_endpoints,
            dns_provider_service,
            acme_provider_service,
            endpoints_provider_service,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        let (ip_provider_sender, ip_provider_reciever) = broadcast::channel(1);
        let (tls_config_sender, tls_config_reciever) = broadcast::channel(1);

        //let (https_ready_sender, https_ready_reciever) = broadcast::channel(1);

        tokio::select! {
            r = self.ip_provider_service.start(ip_provider_sender) => {
                match r {
                    Ok(()) => tracing::debug!("IP Provider exited."),
                    Err(e) => tracing::error!("IP Provider had an error |{}|{}", e, Backtrace::capture())
                }
            }
            r = self.dns_provider_service.start(ip_provider_reciever) => {
                 match r {
                     Ok(()) => tracing::debug!("Cloudflare A/AAAA record service exited."),
                     Err(e) => tracing::error!("Cloudflare A/AAAA had an error |{}|{}", e, Backtrace::capture())
                 }
            }
            r = self.acme_provider_service.start(tls_config_sender) => {
                 match r {
                     Ok(()) => tracing::debug!("Acme Service exited."),
                     Err(e) => tracing::error!("Acme Service had an error |{}|{}", e, Backtrace::capture())
                 }
            }
            r = self.endpoints_provider_service.start(tls_config_reciever) => {
                 match r {
                     Ok(()) => tracing::debug!("Endpoints exited."),
                     Err(e) => tracing::error!("Endpoints had an error |{}|{}", e, Backtrace::capture())
                 }
            }
        }

        Ok(())
    }
}
