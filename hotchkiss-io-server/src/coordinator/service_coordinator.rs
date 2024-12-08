use std::backtrace::Backtrace;

use super::acme_provider_service::AcmeProviderService;
use super::dns_provider_service::DnsProviderService;
use super::endpoints_provider_service::EndpointsProviderService;
use crate::{coordinator::ip_provider_service::IpProviderService, Settings};
/// The goal of the coordinator is to start up the various dependancies of the server AND
/// be able to reconfigure it automatically at runtime.
use anyhow::Result;
use hotchkiss_io_db::DatabaseHandle;
use sqlx::{Pool, Sqlite};
use tokio::sync::broadcast;

pub struct ServiceCoordinator {
    pool: Pool<Sqlite>,
    ip_provider_service: IpProviderService,
    dns_provider_service: DnsProviderService,
    acme_provider_service: AcmeProviderService,
    endpoints_provider_service: EndpointsProviderService,
}

impl ServiceCoordinator {
    pub async fn create(settings: Settings) -> Result<Self> {
        let pool = DatabaseHandle::create(&settings.database_path).await?;

        //let installation_status_service = InstallationStatusService::create(pool.clone());
        let ip_provider_service = IpProviderService::create(settings.omada_config)?;
        //let dns_server_service = DnsServer::create(pool.clone()).await;
        //let install_endpoints = InstallEndpoints::create(pool.clone());
        let dns_provider_service =
            DnsProviderService::create(settings.cloudflare_token.clone(), settings.domain.clone())?;
        let acme_provider_service =
            AcmeProviderService::create(pool.clone(), settings.cloudflare_token, settings.domain)?;
        let endpoints_provider_service = EndpointsProviderService::create(pool.clone())?;

        Ok(Self {
            pool,
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
        //let (install_refresh_sender, install_refresh_reciever) = broadcast::channel(1);
        //let (install_stat_sender, install_stat_reciever) = broadcast::channel(1);
        //let install_stat_reciever2 = install_stat_sender.subscribe();
        let (ip_provider_sender, ip_provider_reciever) = broadcast::channel(1);
        //let ip_provider_reciever2 = ip_provider_sender.subscribe();
        //let install_stat_reciever3 = install_stat_sender.subscribe();
        let (tls_config_sender, tls_config_reciever) = broadcast::channel(1);

        //let (https_ready_sender, https_ready_reciever) = broadcast::channel(1);

        tokio::select! {
            //r = self.installation_status_service.start(install_refresh_reciever, install_stat_sender) => {
            //    match r {
            //        Ok(()) => tracing::debug!("Install Status Service exited."),
            //        Err(e) => tracing::error!("Install Status Service had an error |{}", e)
            //    }
            //}
            r = self.ip_provider_service.start(ip_provider_sender) => {
                match r {
                    Ok(()) => tracing::debug!("IP Provider exited."),
                    Err(e) => tracing::error!("IP Provider had an error |{}|{}", e, Backtrace::capture())
                }
            }
            //r = self.dns_server_service.start() => {
            //     match r {
            //         Ok(()) => tracing::debug!("DNS Server exited."),
            //         Err(e) => tracing::error!("DNS Server had an error |{}", e)
            //     }
            // }
            // r = self.install_endpoints.start(install_stat_reciever, install_refresh_sender) => {
            //     match r {
            //         Ok(()) => tracing::debug!("Install Endpoints exited."),
            //         Err(e) => tracing::error!("Install Endpoints had an error |{}", e)
            //     }
            // }
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
