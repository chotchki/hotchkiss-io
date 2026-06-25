use super::dns::cloudflare_client::CloudflareClient;
use crate::settings::Settings;
use anyhow::Result;
use hickory_resolver::TokioAsyncResolver;
use std::{collections::HashSet, net::IpAddr, sync::Arc, time::Duration};
use tokio::sync::broadcast::{Receiver, error::RecvError};
use tokio::time::{MissedTickBehavior, interval, sleep};
use tracing::{debug, error, info};

/// Re-reconcile DNS on this cadence even without an IP change, so a failed
/// Cloudflare update retries itself (we no longer crash-restart to recover).
const RECONCILE_EVERY: Duration = Duration::new(15 * 60, 0);

/// A service component that updates the dns setup whenever the underlying public ip changes.
pub struct DnsProviderService {
    settings: Arc<Settings>,
    resolver: TokioAsyncResolver,
    client: CloudflareClient,
}

impl DnsProviderService {
    /// Components required to create the provider
    pub fn create(
        settings: Arc<Settings>,
        resolver: TokioAsyncResolver,
        client: CloudflareClient,
    ) -> Self {
        Self {
            settings,
            resolver,
            client,
        }
    }

    /// Runs forever, reconciling Cloudflare DNS with the current public IP. Self-
    /// heals: a transient Cloudflare/lookup error is logged and retried (on the
    /// next IP change or the reconcile tick) rather than `?`-propagated into the
    /// coordinator's `try_join!`, which would take the whole app down.
    pub async fn start(&self, mut ip_changed: Receiver<HashSet<IpAddr>>) -> Result<()> {
        debug!("Waiting for the initial public IP");
        let mut current_ips = loop {
            match ip_changed.recv().await {
                Ok(ips) => break ips,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => {
                    error!("IP channel closed; DNS provider waiting for recovery");
                    sleep(RECONCILE_EVERY).await;
                }
            }
        };

        let mut reconcile = interval(RECONCILE_EVERY);
        reconcile.set_missed_tick_behavior(MissedTickBehavior::Skip);
        reconcile.tick().await; // consume the immediate first tick

        loop {
            let dns_ips = self.lookup_dns().await?;
            if current_ips != dns_ips {
                info!("Updating DNS: have {:?}, want {:?}", dns_ips, current_ips);
                if let Err(e) = self.client.update_dns(current_ips.clone()).await {
                    error!("DNS update failed (will retry): {e:?}");
                }
            }

            // Wake on the next IP change OR the periodic reconcile.
            tokio::select! {
                r = ip_changed.recv() => match r {
                    Ok(ips) => current_ips = ips,
                    Err(RecvError::Lagged(_)) => {} // missed some; re-reconcile
                    Err(RecvError::Closed) => {
                        error!("IP channel closed; DNS provider waiting for recovery");
                        sleep(RECONCILE_EVERY).await;
                    }
                },
                _ = reconcile.tick() => {}
            }
        }
    }

    async fn lookup_dns(&self) -> Result<HashSet<IpAddr>> {
        let lookup_result = match self
            .resolver
            .ipv4_lookup(format!("{}.", self.settings.domain))
            .await
        {
            Ok(o) => o,
            Err(e) => {
                debug!("DNS lookup of {} failed with {}", self.settings.domain, e);
                return Ok(HashSet::new());
            }
        };

        let hash: HashSet<IpAddr> = lookup_result.iter().map(|x| IpAddr::V4(x.0)).collect();

        Ok(hash)
    }
}
