use super::dns::cloudflare_client::CloudflareClient;
use anyhow::Result;
use hickory_resolver::TokioAsyncResolver;
use std::{collections::HashSet, net::IpAddr};
use tokio::sync::broadcast::Receiver;
use tracing::{debug, info};

pub struct DnsProviderService {
    domain: String,
    client: CloudflareClient,
    resolver: TokioAsyncResolver,
}

impl DnsProviderService {
    pub fn create(resolver: TokioAsyncResolver, token: String, domain: String) -> Result<Self> {
        Ok(Self {
            domain: domain.clone(),
            client: CloudflareClient::new(token, domain)?,
            resolver,
        })
    }

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
        let lookup_result = match self.resolver.ipv4_lookup(self.domain.clone() + ".").await {
            Ok(o) => o,
            Err(e) => {
                debug!("DNS lookup of {} failed with {}", self.domain, e);
                return Ok(HashSet::new());
            }
        };

        let hash: HashSet<IpAddr> = lookup_result.iter().map(|x| IpAddr::V4(x.0)).collect();

        Ok(hash)
    }
}
