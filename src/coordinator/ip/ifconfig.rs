//! This module is to discover the server's public ip address for proper external dns setup.
//!
//! Ideally we wouldn't depend on a third party service for this but looking it up locally is extremely slow.
use anyhow::Result;
use reqwest::{Client, ClientBuilder};
use std::{net::Ipv4Addr, str::FromStr};

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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[tokio::test]
    async fn basic_run() -> Result<()> {
        let client = IfconfigMe::new()?;
        let addr = client.public_ip().await?;
        assert!(!addr.is_private());
        Ok(())
    }
}
