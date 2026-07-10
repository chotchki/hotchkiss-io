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
use anyhow::{Result, bail};
use hickory_resolver::TokioAsyncResolver;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{error, info};

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

        let ip_provider_service = IpProviderService::create(settings.static_ip)?;
        let dns_provider_service = DnsProviderService::create(
            settings.clone(),
            resolver.clone(),
            cloudflare_client.clone(),
        );
        let acme_provider_service =
            AcmeProviderService::create(settings.clone(), pool.clone(), cloudflare_client)?;
        // Phase CX: the shared in-memory greylist snapshot, threaded into BOTH the detection
        // sweep (writer) and AppState (reader) so request-path enforcement never hits the DB.
        let greylist_set = crate::greylist::active_set::GreylistSet::new();
        // Phase DL: the shared dead-link scanner handle, threaded into BOTH the daily
        // scan loop and AppState (the "Run scan now" button + status), same pattern.
        let dead_links = crate::deadlinks::DeadLinkScanState::new();
        let endpoints_provider_service = EndpointsProviderService::create(
            settings.clone(),
            pool.clone(),
            greylist_set.clone(),
            resolver.clone(),
            dead_links.clone(),
        )
        .await?;

        // Phase CN: backfill responsive AVIF variants for images uploaded before
        // the pipeline existed. Detached background one-shot (NOT in the try_join)
        // — backup-first + per-item non-fatal, so it can't delay boot or take the
        // app down; idempotent → a no-op once the backlog is cleared.
        super::backfill_responsive_images::spawn(pool.clone(), settings.clone());

        // Phase CR.2: stamp the stored is_bot for request_log rows logged before the
        // column existed. Same detached / non-fatal / idempotent shape.
        super::backfill_is_bot::spawn(pool.clone());

        // Phase CX: the behavioral greylist detection sweep. Detached interval loop (NOT in
        // the try_join!) — a failed pass logs and retries, never takes the app down. Reuses
        // the ACME resolver for FCrDNS crawler verification, and refreshes the shared snapshot
        // the enforcement middleware reads.
        crate::greylist::sweep::spawn(pool.clone(), resolver.clone(), greylist_set);

        // Phase DL: the daily dead-link scan. Detached interval loop (NOT in the
        // try_join!) — a failed pass logs and retries next tick, never takes the app
        // down. Resolves internal links in-DB, checks external over HTTP with per-host
        // politeness, and shares the single-flight handle with the admin trigger. The
        // scan's site_host is the registrable rp_id (same as AppState.site_host), so a
        // same-site absolute link folds to internal on beta as well as prod.
        crate::deadlinks::spawn(pool.clone(), settings.webauthn_rp_id.clone(), dead_links);

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
        info!("IPs Handler Task ID: {}", ips_handle.id());
        let dps_handle = tokio::spawn(async move { dps.start(ip_provider_reciever).await });
        info!("DNS Handler Task ID: {}", dps_handle.id());
        let aps_handle = tokio::spawn(async move { aps.start(tls_config_sender).await });
        info!("ACME Handler Task ID: {}", aps_handle.id());
        let eps_handle = tokio::spawn(async move { eps.start(tls_config_reciever).await });
        info!("Endpoints Handler Task ID: {}", eps_handle.id());

        async fn flatten<T>(handle: JoinHandle<Result<T>>) -> Result<T> {
            let id = handle.id();
            match handle.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(err)) => {
                    error!("Service id {} failed with error {}", id, err);
                    Err(err)
                }
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
