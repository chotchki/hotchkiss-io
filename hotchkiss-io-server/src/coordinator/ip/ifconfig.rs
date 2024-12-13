use std::{net::Ipv4Addr, str::FromStr};

use anyhow::Result;
use reqwest::{Client, ClientBuilder};

#[derive(Debug)]
pub struct IfconfigMe {
    client: Client,
}

impl IfconfigMe {
    pub fn new() -> Result<Self> {
        let builder = ClientBuilder::new().use_rustls_tls();

        Ok(Self {
            client: builder.build()?,
        })
    }

    pub async fn public_ip(&self) -> Result<Ipv4Addr> {
        let address = self
            .client
            .get("https://ifconfig.me/ip")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(Ipv4Addr::from_str(&address)?)
    }
}
