use anyhow::anyhow;
use anyhow::Result;
use reqwest::header;
use std::collections::HashMap;

use reqwest::{Certificate, Client, ClientBuilder, Url};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct OmadaConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

pub struct OmadaClient {
    base: Url,
    client: Client,
    config: OmadaConfig,
    omadac_id: Option<String>,
    token: Option<String>,
}

impl OmadaClient {
    pub fn new(config: OmadaConfig) -> Result<OmadaClient> {
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
            .cookie_store(true)
            .default_headers(headers);

        Ok(OmadaClient {
            base: Url::parse(&config.url)?,
            client: builder.build()?,
            config,
            omadac_id: None,
            token: None,
        })
    }

    pub async fn login(&mut self) -> anyhow::Result<()> {
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
            .error_for_status()?;

        let response_json = response.json::<serde_json::Value>().await?;

        self.omadac_id = Some(
            response_json["result"]["omadacId"]
                .clone()
                .as_str()
                .ok_or_else(|| anyhow!("No omadacId"))?
                .to_string(),
        );
        self.token = Some(
            response_json["result"]["token"]
                .clone()
                .as_str()
                .ok_or_else(|| anyhow!("No Token"))?
                .to_string(),
        );

        Ok(())
    }

    pub async fn get_wan_ip(&self) -> Result<String> {
        let omadac_id = self
            .omadac_id
            .clone()
            .ok_or_else(|| anyhow!("Missing the omadacId"))?;
        let token = self
            .token
            .clone()
            .ok_or_else(|| anyhow!("Missing the token, did you login?"))?;

        let user_info = self.base.join("/api/v2/current/users")?;
        let user_info_response = self
            .client
            .get(user_info.clone())
            .header("Csrf-Token", token.clone())
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let user_info_json: serde_json::Value = serde_json::from_str(&user_info_response)?;

        let site_id = user_info_json["result"]["privilege"]["sites"][0]
            .as_str()
            .ok_or_else(|| anyhow!("Unable to find site id").context(user_info.clone()))?;

        let controller_status = self
            .base
            .join(&("/".to_string() + &omadac_id + "/api/v2/maintenance/controllerStatus"))?;
        let controller_status_response = self
            .client
            .get(controller_status.clone())
            .header("Csrf-Token", token.clone())
            .send()
            .await?
            .error_for_status()?;
        let controller_status_json = controller_status_response
            .json::<serde_json::Value>()
            .await?;

        let controller_reformatted = controller_status_json["result"]["macAddress"]
            .as_str()
            .ok_or_else(|| {
                anyhow!("Unable to find controller MAC").context(controller_status.clone())
            })?
            .replace(":", "-");

        let gateway_info = self.base.join(
            &("/".to_string()
                + &omadac_id
                + "/api/v2/sites/"
                + site_id
                + "/gateways/"
                + &controller_reformatted),
        )?;
        let gateway_info_response = self
            .client
            .get(gateway_info.clone())
            .header("Csrf-Token", token)
            .send()
            .await?
            .error_for_status()?;

        let gateway_info_json = gateway_info_response.json::<serde_json::Value>().await?;
        let port_info = gateway_info_json["result"]["portStats"]
            .as_array()
            .ok_or_else(|| anyhow!("Unable to find gateway ports").context(gateway_info.clone()))?;

        for port in port_info {
            let public_ip = port["wanPortIpv4Config"]["ip"].clone();
            if public_ip.is_string() {
                return Ok(public_ip.as_str().unwrap().to_string());
            }
        }

        Err(anyhow!("Unable to find wan ip").context(gateway_info.clone()))
    }
}
