use anyhow::Result;
use anyhow::bail;
use reqwest::Response;
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::LazyLock;
use tracing::error;
use url::Url;

use crate::settings::Settings;

static BASE_URL: LazyLock<Url> =
    LazyLock::new(|| Url::parse("https://api.cloudflare.com/client/v4/").unwrap());

#[derive(Clone, Debug)]
pub struct CloudflareApi {
    settings: Arc<Settings>,
    client: Client,
}

impl CloudflareApi {
    pub fn new(settings: Arc<Settings>) -> Result<CloudflareApi> {
        let builder = ClientBuilder::new().use_rustls_tls();

        Ok(CloudflareApi {
            settings,
            client: builder.build()?,
        })
    }

    /// `…/zones/{zone}/dns_records` — collection endpoint (create + list).
    fn dns_records_url(zone_id: &str) -> Result<Url> {
        Ok(BASE_URL.join(&format!("./zones/{zone_id}/dns_records"))?)
    }

    /// `…/zones/{zone}/dns_records/{record}` — single-record endpoint (delete).
    fn dns_record_url(zone_id: &str, dns_id: &str) -> Result<Url> {
        Ok(BASE_URL.join(&format!("./zones/{zone_id}/dns_records/{dns_id}"))?)
    }

    /// `…/zones?name={zone_name}` — zone lookup by name.
    fn zones_query_url(zone_name: &str) -> Result<Url> {
        let mut url = BASE_URL.join("./zones")?;
        url.query_pairs_mut().append_pair("name", zone_name);
        Ok(url)
    }

    /// `…/zones/{zone}/dns_records?name={name}&type={rec_type}` — list records
    /// matching a name *and* a record type. `rec_type` is a parameter, not a
    /// constant — pinning the type here once broke ACME cleanup (Phase 1).
    fn dns_records_query_url(zone_id: &str, name: &str, rec_type: &str) -> Result<Url> {
        let mut url = Self::dns_records_url(zone_id)?;
        url.query_pairs_mut()
            .append_pair("name", name)
            .append_pair("type", rec_type);
        Ok(url)
    }

    pub async fn create_record(
        &self,
        zone_id: &ZoneInfoId,
        name: String,
        addr: IpAddr,
    ) -> Result<()> {
        let url = Self::dns_records_url(&zone_id.0)?;

        let content = match addr {
            IpAddr::V4(v4) => json!({
                "ttl": 1,
                "name": name,
                "content": v4.to_string(),
                "type": "A"
            }),
            IpAddr::V6(v6) => json!({
                "ttl": 1,
                "name": name,
                "content": v6.to_string(),
                "type": "AAAA"
            }),
        };

        Self::transform_error(
            self.client
                .post(url)
                .bearer_auth(&self.settings.cloudflare_token)
                .json(&content)
                .send()
                .await?,
        )
        .await?;

        Ok(())
    }

    pub async fn create_txt_record(
        &self,
        zone_id: &ZoneInfoId,
        name: &str,
        value: &str,
    ) -> Result<()> {
        let url = Self::dns_records_url(&zone_id.0)?;

        let content = json!({
            "name": name,
            "content": format!("\"{}\"", value),
            "type": "TXT",
            "ttl": 60
        });

        Self::transform_error(
            self.client
                .post(url)
                .bearer_auth(&self.settings.cloudflare_token)
                .json(&content)
                .send()
                .await?,
        )
        .await?;

        Ok(())
    }

    pub async fn delete_record(&self, zone_id: &ZoneInfoId, dns_id: &DnsRecId) -> Result<()> {
        let url = Self::dns_record_url(&zone_id.0, &dns_id.0)?;

        Self::transform_error(
            self.client
                .delete(url)
                .bearer_auth(&self.settings.cloudflare_token)
                .send()
                .await?,
        )
        .await?;

        Ok(())
    }

    pub async fn get_zone_id(&self, domain: &str) -> Result<ZoneInfoId> {
        let mut suffixes: Vec<&str> = domain.split('.').rev().take(2).collect();
        suffixes.reverse();
        let parent = suffixes[..].join(".");

        let url = Self::zones_query_url(&parent)?;

        let mut response = Self::transform_error(
            self.client
                .get(url)
                .bearer_auth(&self.settings.cloudflare_token)
                .send()
                .await?,
        )
        .await?
        .json::<ResultsZoneInfo>()
        .await?;

        if response.result.is_empty() {
            bail!("No zone id found for {domain}");
        } else {
            Ok(response.result.remove(0).id)
        }
    }

    /// For a given domain and record type, fetch matching DNS records (content + cloudflare id).
    pub async fn get_recs_by_name(
        &self,
        zone_id: &ZoneInfoId,
        domain: &str,
        rec_type: &str,
    ) -> Result<Vec<DnsRec>> {
        let url = Self::dns_records_query_url(&zone_id.0, domain, rec_type)?;

        let response = Self::transform_error(
            self.client
                .get(url)
                .bearer_auth(&self.settings.cloudflare_token)
                .send()
                .await?,
        )
        .await?
        .json::<ResultsDnsRec>()
        .await?;

        Ok(response.result)
    }

    async fn transform_error(response: Response) -> Result<Response> {
        if !response.status().is_success() {
            let request_url = response.url().clone();
            let status_code = response.status();
            let body = response
                .text()
                .await
                .unwrap_or("No response body".to_string());
            error!("Reqwest failed with status code {}", status_code);
            error!("Request url {}", request_url);
            error!("Response body {}", body);
            bail!("Reqwest failed");
        }
        Ok(response)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_records_url_collection() {
        assert_eq!(
            CloudflareApi::dns_records_url("zone123").unwrap().as_str(),
            "https://api.cloudflare.com/client/v4/zones/zone123/dns_records"
        );
    }

    #[test]
    fn dns_record_url_single() {
        assert_eq!(
            CloudflareApi::dns_record_url("zone123", "rec456")
                .unwrap()
                .as_str(),
            "https://api.cloudflare.com/client/v4/zones/zone123/dns_records/rec456"
        );
    }

    #[test]
    fn zones_query_url_by_name() {
        assert_eq!(
            CloudflareApi::zones_query_url("hotchkiss.io")
                .unwrap()
                .as_str(),
            "https://api.cloudflare.com/client/v4/zones?name=hotchkiss.io"
        );
    }

    #[test]
    fn dns_records_query_includes_name_and_type() {
        let url = CloudflareApi::dns_records_query_url(
            "zone123",
            "_acme-challenge.hotchkiss.io",
            "TXT",
        )
        .unwrap();
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            pairs,
            vec![
                ("name".to_string(), "_acme-challenge.hotchkiss.io".to_string()),
                ("type".to_string(), "TXT".to_string()),
            ]
        );
        assert_eq!(url.path(), "/client/v4/zones/zone123/dns_records");
    }

    #[test]
    fn dns_records_query_type_is_a_parameter_not_hardcoded() {
        // Same call, different `rec_type` → the `type=` value tracks the
        // argument. Regression guard for the Phase 1 bug where `type=A`
        // was pinned and `clean_proof`'s TXT lookup silently returned 0 rows.
        for rec_type in ["A", "AAAA", "TXT", "CNAME"] {
            let url = CloudflareApi::dns_records_query_url("z", "example.com", rec_type).unwrap();
            let got_type = url
                .query_pairs()
                .find(|(k, _)| k == "type")
                .map(|(_, v)| v.into_owned());
            assert_eq!(got_type.as_deref(), Some(rec_type));
        }
    }
}
