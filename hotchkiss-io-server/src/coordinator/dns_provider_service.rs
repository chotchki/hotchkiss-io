use super::dns::cloudflare_client::CloudflareClient;
use anyhow::Result;
use std::{collections::HashSet, net::IpAddr};
use tokio::{net::lookup_host, sync::broadcast::Receiver};
use tracing::info;

pub struct DnsProviderService {
    domain: String,
    client: CloudflareClient,
}

impl DnsProviderService {
    pub fn create(token: String, domain: String) -> Result<Self> {
        Ok(Self {
            domain: domain.clone(),
            client: CloudflareClient::new(token, domain)?,
        })
    }

    pub async fn start(&self, mut ip_changed: Receiver<HashSet<IpAddr>>) -> Result<()> {
        let mut current_ips = ip_changed.recv().await?;
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
        let dns_current_ips: HashSet<IpAddr> =
            lookup_host(&self.domain).await?.map(|x| x.ip()).collect();

        Ok(dns_current_ips)
    }
}
