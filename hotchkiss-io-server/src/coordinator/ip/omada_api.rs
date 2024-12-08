use std::{collections::HashMap, net::Ipv4Addr};

use super::omada_config::OmadaConfig;
use anyhow::{bail, Result};
use reqwest::{cookie::Cookie, header, Certificate, Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use url::Url;

pub struct OmadaApi {
    config: OmadaConfig,
    base: Url,
    client: Client,
}

impl OmadaApi {
    pub fn new(config: OmadaConfig) -> Result<OmadaApi> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "accept",
            header::HeaderValue::from_static("application/json"),
        );

        let builder = ClientBuilder::new()
            .add_root_certificate(Certificate::from_pem(include_bytes!("localhost.pem"))?)
            .use_rustls_tls()
            //This is due to omada generating a cert with a crap hostname
            .danger_accept_invalid_hostnames(true)
            .cookie_store(true);

        let base = Url::parse(&config.url)?;

        Ok(OmadaApi {
            config,
            base,
            client: builder.build()?,
        })
    }

    pub async fn login(&mut self) -> anyhow::Result<LoginData> {
        let url = self.base.join("/api/v2/login")?;

        let mut post_body = HashMap::new();
        post_body.insert("username", self.config.username.clone());
        post_body.insert("password", self.config.password.clone());

        let response = self
            .client
            .post(url)
            .json(&post_body)
            .send()
            .await?
            .error_for_status()?
            .json::<LoginResult>()
            .await?;

        Ok(response.result)
    }

    pub async fn get_user_info(&self, login_data: LoginData) -> Result<UserInfo> {
        let user_info = self.base.join("/api/v2/current/users")?;
        let user_info_response = self
            .client
            .get(user_info.clone())
            .header("Csrf-Token", login_data.token.0)
            .send()
            .await?
            .error_for_status()?
            .json::<UserInfoResult>()
            .await?;

        Ok(user_info_response.result)

        //let site_id = user_info_json["result"]["privilege"]["sites"][0]
    }

    pub async fn get_controller_name(&self, login_data: LoginData) -> Result<ControllerName> {
        let controller_status = self.base.join(
            &(format!(
                "/{}/api/v2/maintenance/controllerStatus",
                login_data.omadacId.0
            )),
        )?;
        let controller_status_response = self
            .client
            .get(controller_status.clone())
            .header("Csrf-Token", login_data.token.0)
            .send()
            .await?
            .error_for_status()?
            .json::<ControllerStatusResult>()
            .await?;

        let controller_mac = controller_status_response.result.macAddress;
        let controller_name = controller_mac.0.replace(":", "-");

        Ok(ControllerName(controller_name))
    }

    pub async fn get_wan_ip(
        &self,
        login_data: LoginData,
        site_id: SiteId,
        controller_name: ControllerName,
    ) -> Result<Ipv4Addr> {
        let gateway_info = self.base.join(
            &(format!(
                "/{}/api/v2/sites/{}/gateways/{}",
                login_data.omadacId.0, site_id.0, controller_name.0
            )),
        )?;
        let mut gateway_info_response = self
            .client
            .get(gateway_info.clone())
            .header("Csrf-Token", login_data.token.0)
            .send()
            .await?
            .error_for_status()?
            .json::<GatewayInfoResult>()
            .await?;

        if gateway_info_response.result.portStats.is_empty() {
            bail!("Unable to find wan ip")
        }
        Ok(gateway_info_response
            .result
            .portStats
            .remove(0)
            .wanPortIpv4Config
            .ip)
    }
}

#[derive(Serialize, Deserialize)]
pub struct LoginResult {
    pub result: LoginData,
}

#[allow(non_snake_case)]
#[derive(Serialize, Deserialize, Clone)]
pub struct LoginData {
    pub omadacId: OmadacId,
    pub token: CSRFToken,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct OmadacId(pub String);

#[derive(Serialize, Deserialize, Clone)]
pub struct CSRFToken(pub String);

#[derive(Serialize, Deserialize)]
pub struct UserInfoResult {
    pub result: UserInfo,
}

#[derive(Serialize, Deserialize)]
pub struct UserInfo {
    pub privilege: Privileges,
}

#[derive(Serialize, Deserialize)]
pub struct Privileges {
    pub sites: Vec<SiteId>,
}

#[derive(Serialize, Deserialize)]
pub struct SiteId(pub String);

#[derive(Serialize, Deserialize)]
pub struct ControllerStatusResult {
    pub result: ControllerInfo,
}

#[allow(non_snake_case)]
#[derive(Serialize, Deserialize)]
pub struct ControllerInfo {
    pub macAddress: ControllerName,
}

#[derive(Serialize, Deserialize)]
pub struct ControllerName(pub String);

#[derive(Serialize, Deserialize)]
pub struct GatewayInfoResult {
    pub result: GatewayInfo,
}

#[allow(non_snake_case)]
#[derive(Serialize, Deserialize)]
pub struct GatewayInfo {
    pub portStats: Vec<PortInfo>,
}

#[allow(non_snake_case)]
#[derive(Serialize, Deserialize)]
pub struct PortInfo {
    pub wanPortIpv4Config: PortIpInfo,
}

#[allow(non_snake_case)]
#[derive(Serialize, Deserialize)]
pub struct PortIpInfo {
    pub ip: Ipv4Addr,
}
