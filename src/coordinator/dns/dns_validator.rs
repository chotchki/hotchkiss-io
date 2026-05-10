use std::time::Duration;

use anyhow::{anyhow, Result};
use hickory_resolver::{
    error::{ResolveError, ResolveErrorKind},
    proto::rr::{RData, RecordType},
    TokioAsyncResolver,
};
use tokio::time::{sleep, Instant};
use tracing::debug;

const TIMEOUT: Duration = Duration::from_secs(300);
const BACKOFF: Duration = Duration::from_millis(250);

/// What a single DNS lookup attempt told us, normalized so the decision
/// logic can be tested without a resolver. (A non-`NoRecordsFound` error is
/// fatal and handled before we get here, so it isn't a variant.)
#[derive(Debug, PartialEq)]
enum LookupOutcome {
    Found(Vec<RData>),
    NoRecords,
}

/// Whether the wait loop is satisfied or should poll again.
#[derive(Debug, PartialEq)]
enum WaitStep {
    Done,
    KeepWaiting,
}

fn is_no_records(e: &ResolveError) -> bool {
    matches!(e.kind(), ResolveErrorKind::NoRecordsFound { .. })
}

/// `ensure_exists` is satisfied once the resolved records (order-insensitive)
/// equal what we expect; anything else — wrong set, or no records yet — keeps
/// it waiting.
fn exists_step(mut expected: Vec<RData>, outcome: LookupOutcome) -> WaitStep {
    match outcome {
        LookupOutcome::Found(mut found) => {
            expected.sort();
            found.sort();
            if expected == found {
                WaitStep::Done
            } else {
                WaitStep::KeepWaiting
            }
        }
        LookupOutcome::NoRecords => WaitStep::KeepWaiting,
    }
}

/// `ensure_not_existing` is satisfied only when the lookup reports no records;
/// any records found (even leftovers) keep it waiting.
fn not_existing_step(outcome: LookupOutcome) -> WaitStep {
    match outcome {
        LookupOutcome::Found(_) => WaitStep::KeepWaiting,
        LookupOutcome::NoRecords => WaitStep::Done,
    }
}

#[derive(Clone, Debug)]
pub struct DnsValidator {
    resolver: TokioAsyncResolver,
}

impl DnsValidator {
    pub fn new(resolver: TokioAsyncResolver) -> DnsValidator {
        Self { resolver }
    }

    /// One uncached lookup; maps the result into a `LookupOutcome`, bubbling
    /// any non-`NoRecordsFound` resolver error.
    async fn lookup_once(&self, domain: &str, record_type: RecordType) -> Result<LookupOutcome> {
        debug!("Clearing cache");
        self.resolver.clear_cache();

        debug!("Performing DNS lookup of {domain}");
        match self.resolver.lookup(domain, record_type).await {
            Ok(o) => Ok(LookupOutcome::Found(
                o.iter().map(|x| x.to_owned()).collect(),
            )),
            Err(ref e) if is_no_records(e) => Ok(LookupOutcome::NoRecords),
            Err(e) => {
                debug!("Some other resolver error occurred: {}", e);
                Err(e.into())
            }
        }
    }

    pub async fn ensure_exists(
        &self,
        domain: &str,
        record_type: RecordType,
        values: Vec<RData>,
    ) -> Result<()> {
        //TODO: The timeout loop fails in an awful way and I don't know why
        let _timeout = Instant::now()
            .checked_add(TIMEOUT)
            .ok_or_else(|| anyhow!("Duration too long"))?;

        let mut backoff = BACKOFF;
        let mut count = 1;

        loop {
            let outcome = self.lookup_once(domain, record_type).await?;
            if let LookupOutcome::Found(ref records) = outcome {
                debug!("DNS records so far: {:?} (expecting {:?})", records, values);
            }
            match exists_step(values.clone(), outcome) {
                WaitStep::Done => return Ok(()),
                WaitStep::KeepWaiting => {}
            }

            debug!("Sleeping for {} secs", backoff.as_secs());
            sleep(backoff).await;
            count += 1;
            backoff = BACKOFF.saturating_mul(count);

            //if Instant::now() > timeout {
            //    bail!("The domain {} doesn't exist past timeout", domain);
            //}
        }
    }

    pub async fn ensure_not_existing(&self, domain: &str, record_type: RecordType) -> Result<()> {
        //TODO: The timeout loop fails in an awful way and I don't know why
        let _timeout = Instant::now()
            .checked_add(TIMEOUT)
            .ok_or_else(|| anyhow!("Duration too long"))?;

        let mut backoff = BACKOFF;
        let mut count = 1;

        loop {
            let outcome = self.lookup_once(domain, record_type).await?;
            match not_existing_step(outcome) {
                WaitStep::Done => return Ok(()),
                WaitStep::KeepWaiting => {}
            }

            debug!("Sleeping for {} secs", backoff.as_secs());
            sleep(backoff).await;
            count += 1;
            backoff = BACKOFF.saturating_mul(count);

            //if Instant::now() > timeout {
            //    bail!("The domain {} still exists past the timeout", domain);
            //}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_resolver::proto::rr::rdata::{A, TXT};
    use std::net::Ipv4Addr;

    fn txt(s: &str) -> RData {
        RData::TXT(TXT::new(vec![s.to_string()]))
    }

    fn a(s: &str) -> RData {
        RData::A(A(s.parse::<Ipv4Addr>().unwrap()))
    }

    #[test]
    fn exists_done_on_exact_match() {
        assert_eq!(
            exists_step(vec![txt("proof")], LookupOutcome::Found(vec![txt("proof")])),
            WaitStep::Done
        );
    }

    #[test]
    fn exists_done_regardless_of_order() {
        assert_eq!(
            exists_step(
                vec![a("1.2.3.4"), a("5.6.7.8")],
                LookupOutcome::Found(vec![a("5.6.7.8"), a("1.2.3.4")]),
            ),
            WaitStep::Done
        );
    }

    #[test]
    fn exists_keeps_waiting_on_wrong_records() {
        assert_eq!(
            exists_step(vec![txt("want")], LookupOutcome::Found(vec![txt("stale")])),
            WaitStep::KeepWaiting
        );
    }

    #[test]
    fn exists_keeps_waiting_on_partial_set() {
        assert_eq!(
            exists_step(
                vec![a("1.2.3.4"), a("5.6.7.8")],
                LookupOutcome::Found(vec![a("1.2.3.4")]),
            ),
            WaitStep::KeepWaiting
        );
    }

    #[test]
    fn exists_keeps_waiting_when_no_records() {
        assert_eq!(
            exists_step(vec![txt("proof")], LookupOutcome::NoRecords),
            WaitStep::KeepWaiting
        );
    }

    #[test]
    fn not_existing_done_only_when_empty() {
        assert_eq!(not_existing_step(LookupOutcome::NoRecords), WaitStep::Done);
    }

    #[test]
    fn not_existing_keeps_waiting_while_records_remain() {
        // The Phase 1 symptom: leftover TXT records that never get deleted
        // (because the cleanup query was pinned to `type=A`) keep this
        // waiting forever. The decision here is correct in isolation — the
        // bug was upstream, in the record-deletion query.
        assert_eq!(
            not_existing_step(LookupOutcome::Found(vec![txt("leftover")])),
            WaitStep::KeepWaiting
        );
    }
}
