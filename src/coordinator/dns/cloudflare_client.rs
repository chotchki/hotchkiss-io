use crate::settings::Settings;

use super::cloudflare_api::CloudflareApi;
use super::cloudflare_api::DnsRec;
use super::cloudflare_api::DnsRecId;
use super::dns_validator::DnsValidator;
use anyhow::Result;
use hickory_resolver::proto::rr::rdata::A;
use hickory_resolver::proto::rr::rdata::TXT;
use hickory_resolver::proto::rr::RData;
use hickory_resolver::proto::rr::RecordType;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;
use tracing::debug;

#[derive(Clone, Debug)]
pub struct CloudflareClient {
    api: CloudflareApi,
    settings: Arc<Settings>,
    dns_validator: DnsValidator,
}

impl CloudflareClient {
    pub fn new(settings: Arc<Settings>, dns_validator: DnsValidator) -> Result<CloudflareClient> {
        Ok(CloudflareClient {
            api: CloudflareApi::new(settings.clone())?,
            settings,
            dns_validator,
        })
    }

    pub async fn clean_proof(&self, proof_domain: &str) -> Result<()> {
        let zone_id = self.api.get_zone_id(&self.settings.domain).await?;

        debug!("Deleting any old cloudflare records for {proof_domain}");
        let old_recs = self.api.get_recs_by_name(&zone_id, proof_domain).await?;
        for r in old_recs {
            self.api.delete_record(&zone_id, &r.id).await?;
        }

        self.dns_validator
            .ensure_not_existing(proof_domain, RecordType::TXT)
            .await?;

        Ok(())
    }

    pub async fn create_proof(&self, proof_domain: &str, proof_value: &str) -> Result<()> {
        let zone_id = self.api.get_zone_id(&self.settings.domain).await?;

        self.clean_proof(proof_domain).await?;

        debug!("Creating cloudflare txt record for {proof_domain} with value {proof_value}");
        self.api
            .create_txt_record(&zone_id, proof_domain, proof_value)
            .await?;

        debug!("Checking the api worked");
        self.dns_validator
            .ensure_exists(
                proof_domain,
                RecordType::TXT,
                vec![RData::TXT(TXT::new(vec![proof_value.to_string()]))],
            )
            .await?;

        Ok(())
    }

    pub async fn update_dns(&self, addrs: HashSet<IpAddr>) -> Result<()> {
        let zone_id = self.api.get_zone_id(&self.settings.domain).await?;

        let dns_recs: Vec<DnsRec> = self
            .api
            .get_recs_by_name(&zone_id, &self.settings.domain)
            .await?;

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
        let mut prefixes: Vec<&str> = self.settings.domain.split('.').rev().skip(2).collect();
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

        //Now validate the propogation
        let records = addrs
            .into_iter()
            .filter_map(|x| match x {
                IpAddr::V4(v4) => Some(RData::A(A(v4))),
                IpAddr::V6(_) => None,
            })
            .collect();

        self.dns_validator
            .ensure_exists(&self.settings.domain, RecordType::A, records)
            .await?;

        Ok(())
    }
}
