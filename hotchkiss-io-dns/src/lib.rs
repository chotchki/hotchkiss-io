use anyhow::Result;
use cloudflare_api::CloudflareApi;
use cloudflare_api::DnsRec;
use cloudflare_api::DnsRecId;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;

mod cloudflare_api;

pub struct CloudflareClient {
    api: CloudflareApi,
    pub token: String,
    pub domain: String,
}

impl CloudflareClient {
    pub async fn new(token: String, domain: String) -> Result<CloudflareClient> {
        Ok(CloudflareClient {
            api: CloudflareApi::new(token.clone())?,
            token,
            domain,
        })
    }

    pub async fn update_dns(&self, addrs: HashSet<IpAddr>) -> Result<()> {
        let zone_id = self.api.get_zone_id(&self.domain).await?;

        let dns_recs: Vec<DnsRec> = self.api.get_recs_by_name(&zone_id, &self.domain).await?;

        let mut dns_ip_to_id: HashMap<IpAddr, DnsRecId> =
            dns_recs.iter().map(|x| (x.content, x.id.clone())).collect();

        //Now we need to figure out, the sets of actions to take
        let mut missing_recs = addrs.clone();
        missing_recs.retain(|x| !dns_ip_to_id.contains_key(x));

        dns_ip_to_id.retain(|k, _| !addrs.contains(k));

        //Now we create and delete records
        for rec in missing_recs {
            self.api
                .create_record(&zone_id, self.domain.clone(), rec)
                .await?;
        }
        for id in dns_ip_to_id.values() {
            self.api.delete_record(&zone_id, id).await?;
        }

        Ok(())
    }
}
