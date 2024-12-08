use anyhow::bail;
use anyhow::Result;
use std::net::Ipv4Addr;

use super::omada_api::OmadaApi;
use super::omada_config::OmadaConfig;

pub struct OmadaClient {
    api: OmadaApi,
}

impl OmadaClient {
    pub fn new(config: OmadaConfig) -> Result<OmadaClient> {
        Ok(OmadaClient {
            api: OmadaApi::new(config)?,
        })
    }

    pub async fn get_wan_ip(&mut self) -> Result<Ipv4Addr> {
        let login_data = self.api.login().await?;
        let mut user_info = self.api.get_user_info(login_data.clone()).await?;

        if user_info.privilege.sites.is_empty() {
            bail!("No sites configured");
        }

        let site_id = user_info.privilege.sites.remove(0);

        let controller_name = self.api.get_controller_name(login_data.clone()).await?;

        let wan_ip = self
            .api
            .get_wan_ip(login_data, site_id, controller_name)
            .await?;

        Ok(wan_ip)
    }
}
