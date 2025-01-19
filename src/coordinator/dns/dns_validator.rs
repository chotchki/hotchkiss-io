use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use hickory_resolver::{
    error::ResolveErrorKind,
    proto::rr::{RData, RecordType},
    TokioAsyncResolver,
};
use tokio::time::{sleep, Instant};
use tracing::debug;

const TIMEOUT: Duration = Duration::from_secs(300);
const BACKOFF: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct DnsValidator {
    resolver: TokioAsyncResolver,
}

impl DnsValidator {
    pub fn new(resolver: TokioAsyncResolver) -> DnsValidator {
        Self { resolver }
    }

    pub async fn ensure_exists(
        &self,
        domain: &str,
        record_type: RecordType,
        mut values: Vec<RData>,
    ) -> Result<()> {
        let timeout = Instant::now()
            .checked_add(TIMEOUT)
            .ok_or_else(|| anyhow!("Duration too long"))?;

        let mut backoff = BACKOFF;
        let mut count = 1;

        values.sort();

        loop {
            debug!("Clearing cache");
            self.resolver.clear_cache();

            debug!("Performing DNS lookup");
            match self.resolver.lookup(domain, record_type).await {
                Ok(o) => {
                    let mut records: Vec<RData> = o.iter().map(|x| x.to_owned()).collect();
                    records.sort();

                    if values == records {
                        return Ok(());
                    } else {
                        debug!(
                            "DNS records don't match yet expected {:?} got {:?}",
                            values, records
                        );
                    }
                }
                Err(ref e)
                    if matches!(
                        e.kind(),
                        ResolveErrorKind::NoRecordsFound {
                            query: _,
                            soa: _,
                            negative_ttl: _,
                            response_code: _,
                            trusted: _
                        }
                    ) =>
                {
                    debug!("No records for {domain} found.");
                }
                Err(e) => {
                    debug!("Some other resolver error occurred: {}", e);
                    return Err(e.into());
                }
            }

            debug!("Sleeping for {} secs", backoff.as_secs());
            sleep(backoff).await;
            count += 1;
            backoff = BACKOFF.saturating_mul(count);
            debug!("Sleep complete 1");

            //if timeout > Instant::now() {
            //    bail!("The domain {} doesn't exist past timeout", domain);
            //}

            debug!("Looping? 1");
        }
    }

    pub async fn ensure_not_existing(&self, domain: &str, record_type: RecordType) -> Result<()> {
        let timeout = Instant::now()
            .checked_add(TIMEOUT)
            .ok_or_else(|| anyhow!("Duration too long"))?;

        let mut backoff = BACKOFF;
        let mut count = 1;

        loop {
            debug!("Clearing cache");
            self.resolver.clear_cache();

            debug!("Performing DNS lookup of {domain}");
            match self.resolver.lookup(domain, record_type).await {
                Ok(r) => {
                    debug!("Records found {:?}, will check again.", r.records());
                }
                Err(ref e)
                    if matches!(
                        e.kind(),
                        ResolveErrorKind::NoRecordsFound {
                            query: _,
                            soa: _,
                            negative_ttl: _,
                            response_code: _,
                            trusted: _
                        }
                    ) =>
                {
                    debug!("No records for {domain} found.");
                    return Ok(());
                }
                Err(e) => {
                    debug!("Some other resolver error occurred: {}", e);
                    return Err(e.into());
                }
            }

            debug!("Sleeping for {} secs", backoff.as_secs());
            sleep(backoff).await;
            count += 1;
            backoff = BACKOFF.saturating_mul(count);
            debug!("Sleep complete 2");

            //if timeout > Instant::now() {
            //    bail!("The domain {} still exists past the timeout", domain);
            //q}

            debug!("Looping? 2");
        }
    }
}
