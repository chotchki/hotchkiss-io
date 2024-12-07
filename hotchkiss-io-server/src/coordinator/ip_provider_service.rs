use anyhow::Result;
use hotchkiss_io_ip::{OmadaClient, OmadaConfig};
use std::{
    collections::HashSet,
    net::{IpAddr, Ipv6Addr},
    time::Duration,
};

use tokio::{
    sync::broadcast::Sender,
    time::{interval, MissedTickBehavior},
};

pub struct IpProviderService {
    client: OmadaClient,
}

impl IpProviderService {
    pub fn create(config: OmadaConfig) -> Result<IpProviderService> {
        Ok(IpProviderService {
            client: OmadaClient::new(config)?,
        })
    }

    /// This schedules a task to periodically wake up and see
    /// if the IP addresses for the machine have changed, if so
    /// they are broadcoast
    pub async fn start(&self, ip_changed: Sender<HashSet<IpAddr>>) -> Result<()> {
        let mut duration = interval(Duration::from_millis(60 * 60 * 1000));
        duration.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut current_ips = Self::server_ips()?;

        //Always send the starting IPs
        tracing::debug!("Sending Initial IP addresses");
        ip_changed.send(current_ips.clone()).ok();

        loop {
            duration.tick().await;

            let new_ips = Self::server_ips()?;
            if current_ips != new_ips {
                current_ips = new_ips;
                tracing::info!("IP addresses changed, broadcasting");
                ip_changed.send(current_ips.clone()).ok();
            }
        }
    }

    /// This function is to figure out what ip addresses should be used to serve HMDL
    fn server_ips() -> Result<Vec<IpAddr>> {
        //let mut omada_client = OmadaClient::new(settings.omada_config)?;
        //omada_client.login().await?;
        //let public_ips = vec![IpAddr::V4(omada_client.get_wan_ip().await?)];

        let addrs = list_afinet_netifas()?;

        let mut filtered_addrs = HashSet::new();

        for (_, addr) in addrs {
            if let IpAddr::V4(addrv4) = addr {
                if !addrv4.is_link_local() {
                    filtered_addrs.insert(IpAddr::V4(addrv4));
                }
            } else if let IpAddr::V6(addrv6) = addr {
                if !Self::has_unicast_link_local_scope(addrv6) {
                    filtered_addrs.insert(IpAddr::V6(addrv6));
                }
            }
        }

        Ok(filtered_addrs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiple_ips() -> Result<(), Box<dyn std::error::Error>> {
        let ips = IpProvderService::server_ips()?;
        assert!(!ips.is_empty());
        Ok(())
    }
}
