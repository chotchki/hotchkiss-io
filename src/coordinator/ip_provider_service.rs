use super::ip::ifconfig::IfconfigMe;
use anyhow::Result;
use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};
use tokio::{
    sync::broadcast::Sender,
    time::{interval, MissedTickBehavior},
};
use tracing::debug;

#[derive(Debug)]
pub struct IpProviderService {
    client: IfconfigMe,
}

impl IpProviderService {
    pub fn create() -> Result<IpProviderService> {
        Ok(IpProviderService {
            client: IfconfigMe::new()?,
        })
    }

    /// This schedules a task to periodically wake up and see
    /// if the IP addresses for the machine have changed, if so
    /// they are broadcoast
    pub async fn start(&self, ip_changed: Sender<HashSet<IpAddr>>) -> Result<()> {
        let mut duration = interval(Duration::from_millis(60 * 60 * 1000));
        duration.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let mut current_ips = self.server_ips().await?;

        //Always send the starting IPs
        debug!("Sending Initial IP addresses {:?}", current_ips);
        ip_changed.send(current_ips.clone()).ok();

        loop {
            duration.tick().await;

            match self.server_ips().await {
                Ok(new_ips) => {
                    if current_ips != new_ips {
                        current_ips = new_ips;
                        tracing::info!("IP addresses changed, broadcasting {:?}", current_ips);
                        ip_changed.send(current_ips.clone()).ok();
                    }
                }
                Err(e) => {
                    tracing::error!("Had an error getting the ip address, not changing {}", e)
                }
            }
        }
    }

    async fn server_ips(&self) -> Result<HashSet<IpAddr>> {
        let current_ip = if cfg!(debug_assertions) {
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        } else {
            IpAddr::V4(self.client.public_ip().await?)
        };

        let mut current_ips = HashSet::new();
        current_ips.insert(current_ip);
        Ok(current_ips)
    }
}
