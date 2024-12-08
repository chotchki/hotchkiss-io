use super::dns::cloudflare_client::CloudflareClient;
use anyhow::Result;
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
};
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
        let hosts: Vec<SocketAddr> = lookup_host(self.domain.to_string() + ":443")
            .await?
            .collect();

        let hash: HashSet<IpAddr> = hosts.iter().map(|x| x.ip()).collect();

        Ok(hash)
    }
}
