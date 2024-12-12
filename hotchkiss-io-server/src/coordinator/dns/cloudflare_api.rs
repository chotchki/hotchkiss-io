use anyhow::bail;
use anyhow::Result;
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::IpAddr;
use std::sync::LazyLock;
use url::Url;

static BASE_URL: LazyLock<Url> =
    LazyLock::new(|| Url::parse("https://api.cloudflare.com/client/v4/").unwrap());

#[derive(Debug)]
pub struct CloudflareApi {
    token: String,
    client: Client,
}

impl CloudflareApi {
    pub fn new(token: String) -> Result<CloudflareApi> {
        let builder = ClientBuilder::new().use_rustls_tls();

        Ok(CloudflareApi {
            token,
            client: builder.build()?,
        })
    }

    pub async fn create_record(
        &self,
        zone_id: &ZoneInfoId,
        name: String,
        addr: IpAddr,
    ) -> Result<()> {
        let url = BASE_URL.join(&format!("./zones/{}/dns_records", zone_id.0))?;

        let content = match addr {
            IpAddr::V4(v4) => json!({
                "ttl": "1",
                "name": name,
                "content": v4.to_string(),
                "type": "A"
            }),
            IpAddr::V6(v6) => json!({
                "ttl": "1",
                "name": name,
                "content": v6.to_string(),
                "type": "AAAA"
            }),
        };

        self.client
            .post(url)
            .bearer_auth(self.token.clone())
            .json(&content)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    pub async fn create_txt_record(
        &self,
        zone_id: &ZoneInfoId,
        name: &str,
        value: &str,
    ) -> Result<()> {
        let url = BASE_URL.join(&format!("./zones/{}/dns_records", zone_id.0))?;

        let content = json!({
            "name": name,
            "content": value,
            "type": "TXT"
        });

        self.client
            .post(url)
            .bearer_auth(self.token.clone())
            .json(&content)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    pub async fn delete_record(&self, zone_id: &ZoneInfoId, dns_id: &DnsRecId) -> Result<()> {
        let url = BASE_URL.join(&format!("./zones/{}/dns_records/{}", zone_id.0, dns_id.0))?;

        self.client
            .delete(url)
            .bearer_auth(self.token.clone())
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    pub async fn get_zone_id(&self, domain: &str) -> Result<ZoneInfoId> {
        let mut suffixes: Vec<&str> = domain.split('.').rev().take(2).collect();
        suffixes.reverse();
        let parent = suffixes[..].join(".");

        let mut url = BASE_URL.join("./zones")?;
        url.set_query(Some(&(format!("name={parent}"))));

        let mut response = self
            .client
            .get(url)
            .bearer_auth(self.token.clone())
            .send()
            .await?
            .error_for_status()?
            .json::<ResultsZoneInfo>()
            .await?;

        if response.result.is_empty() {
            bail!("No zone id found for {domain}");
        } else {
            Ok(response.result.remove(0).id)
        }
    }

    /// For a given domain get the ip addresses AND cloudflare id
    pub async fn get_recs_by_name(
        &self,
        zone_id: &ZoneInfoId,
        domain: &str,
    ) -> Result<Vec<DnsRec>> {
        let mut url = BASE_URL.join(&format!("./zones/{}/dns_records", zone_id.0))?;
        url.set_query(Some(&(format!("name={domain}"))));

        let response = self
            .client
            .get(url.clone())
            .bearer_auth(self.token.clone())
            .send()
            .await?
            .error_for_status()?
            .json::<ResultsDnsRec>()
            .await?;

        Ok(response.result)
    }
}

#[derive(Serialize, Deserialize)]
pub struct ResultsZoneInfo {
    pub result: Vec<ZoneInfo>,
}

#[derive(Serialize, Deserialize)]
pub struct ZoneInfo {
    pub id: ZoneInfoId,
}

#[derive(Serialize, Deserialize)]
pub struct ZoneInfoId(pub String);

#[derive(Serialize, Deserialize)]
pub struct ResultsDnsRec {
    pub result: Vec<DnsRec>,
}

#[derive(Serialize, Deserialize)]
pub struct DnsRec {
    pub content: String,
    pub id: DnsRecId,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DnsRecId(pub String);
