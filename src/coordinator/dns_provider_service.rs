use super::dns::cloudflare_client::CloudflareClient;
use crate::settings::Settings;
use anyhow::Result;
use hickory_resolver::TokioAsyncResolver;
use std::{collections::HashSet, net::IpAddr, sync::Arc};
use tokio::sync::broadcast::Receiver;
use tracing::{debug, info};

/// A service component that updates the dns setup whenever the underlying public ip changes.
pub struct DnsProviderService {
    settings: Arc<Settings>,
    resolver: TokioAsyncResolver,
    client: CloudflareClient,
}

impl DnsProviderService {
    /// Components required to create the provider
    pub fn create(
        settings: Arc<Settings>,
        resolver: TokioAsyncResolver,
        client: CloudflareClient,
    ) -> Self {
        Self {
            settings,
            resolver,
            client,
        }
    }

    /// This starts the provider running and will not return except on errors
    pub async fn start(&self, mut ip_changed: Receiver<HashSet<IpAddr>>) -> Result<()> {
        let mut current_ips = ip_changed.recv().await?;

        debug!("Got ip address, checking dns");
        let mut dns_ips = self.lookup_dns().await?;

        loop {
            if current_ips != dns_ips {
                //Need to update the dns
                info!("Updating DNS");
                self.client.update_dns(current_ips).await?;
            }

            //Wait for changes
            current_ips = ip_changed.recv().await?;
            dns_ips = self.lookup_dns().await?;
        }
    }

    async fn lookup_dns(&self) -> Result<HashSet<IpAddr>> {
        let lookup_result = match self
            .resolver
            .ipv4_lookup(format!("{}.", self.settings.domain))
            .await
        {
            Ok(o) => o,
            Err(e) => {
                debug!("DNS lookup of {} failed with {}", self.settings.domain, e);
                return Ok(HashSet::new());
            }
        };

        let hash: HashSet<IpAddr> = lookup_result.iter().map(|x| IpAddr::V4(x.0)).collect();

        Ok(hash)
    }
}
