/// The goal of the coordinator is to start up the various dependancies of the server AND
/// be able to reconfigure it automatically at runtime.
use anyhow::Result;

mod dns;
mod ip;
mod ip_provider_service;
use crate::{coordinator::ip_provider_service::IpProviderService, Settings};

pub struct Coordinator {
    ip_provider_service: IpProviderService,
}

impl Coordinator {
    pub async fn create(settings: Settings) -> Result<Coordinator> {
        //let pool = DatabaseHandle::create(path).await?;

        //let installation_status_service = InstallationStatusService::create(pool.clone());
        let ip_provider_service = IpProviderService::create(settings.omada_config);
        //let dns_server_service = DnsServer::create(pool.clone()).await;
        //let install_endpoints = InstallEndpoints::create(pool.clone());
        //let cloudflare_a_service = CloudflareAService::create();
        //let acme_provision_service = AcmeProvisionService::create(pool.clone()).await;
        //let endpoints = Endpoints::create(pool.clone(), rand_gen.clone())?;

        Ok(Self {
            //installation_status_service,
            ip_provider_service,
            //dns_server_service,
            //install_endpoints,
            //cloudflare_a_service,
            //acme_provision_service,
            //endpoints,
        })
    }

    pub async fn start(&mut self) -> Result<(), CoordinatorError> {
        let (install_refresh_sender, install_refresh_reciever) = broadcast::channel(1);
        let (install_stat_sender, install_stat_reciever) = broadcast::channel(1);
        let install_stat_reciever2 = install_stat_sender.subscribe();
        let (ip_provider_sender, ip_provider_reciever) = broadcast::channel(1);
        //let ip_provider_reciever2 = ip_provider_sender.subscribe();
        let install_stat_reciever3 = install_stat_sender.subscribe();
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
                    Err(e) => tracing::error!("IP Provider had an error |{}", e)
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
            // r = self.cloudflare_a_service.start(ip_provider_reciever, install_stat_reciever2) => {
            //     match r {
            //         Ok(()) => tracing::debug!("Cloudflare A/AAAA record service exited."),
            //         Err(e) => tracing::error!("Cloudflare A/AAAA had an error |{}", e)
            //     }
            // }
            // r = self.acme_provision_service.start(install_stat_reciever3, tls_config_sender) => {
            //     match r {
            //         Ok(()) => tracing::debug!("Acme Service exited."),
            //         Err(e) => tracing::error!("Acme Service had an error |{}", e)
            //     }
            // }
            // r = self.endpoints.start(tls_config_reciever) => {
            //     match r {
            //         Ok(()) => tracing::debug!("Endpoints exited."),
            //         Err(e) => tracing::error!("Endpoints had an error |{}", e)
            //     }
            // }
        }

        Ok(())
    }
}
