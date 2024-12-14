use super::cloudflare_api::CloudflareApi;
use super::cloudflare_api::DnsRec;
use super::cloudflare_api::DnsRecId;
use anyhow::Result;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::str::FromStr;

#[derive(Debug)]
pub struct CloudflareClient {
    api: CloudflareApi,
    pub domain: String,
}

impl CloudflareClient {
    pub fn new(token: String, domain: String) -> Result<CloudflareClient> {
        Ok(CloudflareClient {
            api: CloudflareApi::new(token.clone())?,
            domain,
        })
    }

    pub async fn create_proof(&self, proof_name: &str, proof_value: &str) -> Result<()> {
        let zone_id = self.api.get_zone_id(&self.domain).await?;

        let old_recs = self.api.get_recs_by_name(&zone_id, proof_name).await?;
        for r in old_recs {
            self.api.delete_record(&zone_id, &r.id).await?;
        }

        self.api
            .create_txt_record(&zone_id, proof_name, proof_value)
            .await?;

        Ok(())
    }

    pub async fn update_dns(&self, addrs: HashSet<IpAddr>) -> Result<()> {
        let zone_id = self.api.get_zone_id(&self.domain).await?;

        let dns_recs: Vec<DnsRec> = self.api.get_recs_by_name(&zone_id, &self.domain).await?;

        let mut dns_ip_to_id: HashMap<IpAddr, DnsRecId> = HashMap::new();
        for rec in dns_recs {
            dns_ip_to_id.insert(
                IpAddr::V4(Ipv4Addr::from_str(&rec.content)?),
                rec.id.clone(),
            );
        }

        //Now we need to figure out, the sets of actions to take
        let mut missing_recs = addrs.clone();
        missing_recs.retain(|x| !dns_ip_to_id.contains_key(x));

        dns_ip_to_id.retain(|k, _| !addrs.contains(k));

        //Now get record name needed
        let mut prefixes: Vec<&str> = self.domain.split('.').rev().skip(2).collect();
        prefixes.reverse();
        let prefix = prefixes[..].join(".");

        //Now we create and delete records
        for rec in missing_recs {
            self.api
                .create_record(&zone_id, prefix.clone(), rec)
                .await?;
        }
        for id in dns_ip_to_id.values() {
            self.api.delete_record(&zone_id, id).await?;
        }

        Ok(())
    }
}
