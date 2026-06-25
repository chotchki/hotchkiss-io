//! Discovers the server's public IPv4 address via Cloudflare's `cdn-cgi/trace`
//! endpoint. We already depend on Cloudflare for DNS, so this adds no new
//! third-party dependency (it replaces the old `ifconfig.me` lookup).
//!
//! `https://1.1.1.1/cdn-cgi/trace` returns `key=value\n` lines; the `ip=`
//! line is the requester's public address as Cloudflare sees it. Connecting
//! to the IPv4 literal `1.1.1.1` forces an IPv4 path, so `ip=` is always v4.
use anyhow::{Context, Result};
use reqwest::{Client, ClientBuilder};
use std::net::Ipv4Addr;
use std::time::Duration;
use tracing::debug;

#[derive(Debug)]
pub struct CloudflareTrace {
    client: Client,
}

impl CloudflareTrace {
    pub fn new() -> Result<Self> {
        let builder = ClientBuilder::new()
            .use_rustls_tls()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30));

        Ok(Self {
            client: builder.build()?,
        })
    }

    pub async fn public_ip(&self) -> Result<Ipv4Addr> {
        let body = self
            .client
            .get("https://1.1.1.1/cdn-cgi/trace")
            .send()
            .await?
            .error_for_status()
            .context("cdn-cgi/trace request returned an error status")?
            .text()
            .await?;

        Self::parse_ip(&body)
    }

    fn parse_ip(body: &str) -> Result<Ipv4Addr> {
        let value = body
            .lines()
            .find_map(|line| line.strip_prefix("ip="))
            .context("cdn-cgi/trace response had no `ip=` line — Cloudflare changed the format?")?;

        debug!("cdn-cgi/trace reported ip={value}");
        value
            .parse::<Ipv4Addr>()
            .with_context(|| format!("cdn-cgi/trace `ip=` value `{value}` is not a valid IPv4 address"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
fl=412f30
h=1.1.1.1
ip=203.0.113.42
ts=1718053200.123
visit_scheme=https
uag=curl/8.4.0
colo=SJC
sliver=none
http=http/2
loc=US
tls=TLSv1.3
sni=plaintext
warp=off
gateway=off
rbi=off
kex=X25519
";

    #[test]
    fn parses_ip_from_sample() {
        let ip = CloudflareTrace::parse_ip(SAMPLE).unwrap();
        assert_eq!(ip, Ipv4Addr::new(203, 0, 113, 42));
    }

    #[test]
    fn missing_ip_line_errors() {
        let body = "fl=abc\nh=1.1.1.1\nts=1.2\n";
        assert!(CloudflareTrace::parse_ip(body).is_err());
    }

    #[test]
    fn malformed_ip_value_errors() {
        assert!(CloudflareTrace::parse_ip("ip=not-an-ip\n").is_err());
    }

    #[tokio::test]
    async fn basic_run() -> Result<()> {
        let client = CloudflareTrace::new()?;
        let addr = client.public_ip().await?;
        assert!(!addr.is_private());
        Ok(())
    }
}
