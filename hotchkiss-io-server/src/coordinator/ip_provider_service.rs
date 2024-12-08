use super::ip::{omada_client::OmadaClient, omada_config::OmadaConfig};
use anyhow::Result;
use std::{collections::HashSet, net::IpAddr, time::Duration};
use tokio::{
    sync::broadcast::Sender,
    time::{interval, MissedTickBehavior},
};
use tracing::{debug, instrument};

#[derive(Debug)]
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
    #[instrument]
    pub async fn start(&mut self, ip_changed: Sender<HashSet<IpAddr>>) -> Result<()> {
        let mut duration = interval(Duration::from_millis(60 * 60 * 1000));
        duration.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let mut current_ips = self.server_ips().await?;

        //Always send the starting IPs
        debug!("Sending Initial IP addresses");
        ip_changed.send(current_ips.clone()).ok();

        loop {
            duration.tick().await;

            match self.server_ips().await {
                Ok(new_ips) => {
                    if current_ips != new_ips {
                        current_ips = new_ips;
                        tracing::info!("IP addresses changed, broadcasting");
                        ip_changed.send(current_ips.clone()).ok();
                    }
                }
                Err(e) => {
                    tracing::error!("Had an error getting the ip address, not changing {}", e)
                }
            }
        }
    }

    #[instrument]
    async fn server_ips(&mut self) -> Result<HashSet<IpAddr>> {
        let current_ip = IpAddr::V4(self.client.get_wan_ip().await?);
        let mut current_ips = HashSet::new();
        current_ips.insert(current_ip);
        Ok(current_ips)
    }
}
