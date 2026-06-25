use super::dns::cloudflare_client::CloudflareClient;
use crate::settings::Settings;
use crate::{
    coordinator::acme::{certificate_loader::CertificateLoader, instant_acme::InstantAcmeDomain},
    db::dao::certificate::CertificateDao,
};
use anyhow::Result;
use axum_server::tls_rustls::RustlsConfig;
use rustls::ServerConfig;
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};
use tokio::{sync::broadcast::Sender, time::sleep};
use tracing::{debug, error, info};

const CERT_REFRESH: Duration = Duration::new(6 * 60 * 60, 0);
/// Conservative retry on failure. Stays under Let's Encrypt's failed-validation
/// rate limit (~5/hour per account+host) so a stuck order can't crash-loop the
/// app into a rate-limit ban (the failure mode the old `?`-propagation risked).
const RETRY_BACKOFF: Duration = Duration::new(15 * 60, 0);

#[derive(Debug)]
pub struct AcmeProviderService {
    settings: Arc<Settings>,
    pool: SqlitePool,
    client: CloudflareClient,
}

impl AcmeProviderService {
    pub fn create(
        settings: Arc<Settings>,
        pool: SqlitePool,
        client: CloudflareClient,
    ) -> Result<Self> {
        Ok(Self {
            settings,
            pool,
            client,
        })
    }

    pub async fn start(&self, tls_config_sender: Sender<RustlsConfig>) -> Result<()> {
        let mut current_certificate: Option<CertificateLoader> = None;
        loop {
            if let Err(e) = self
                .refresh_once(&mut current_certificate, &tls_config_sender)
                .await
            {
                // A transient LE/Cloudflare failure must NOT bubble into the
                // coordinator's `try_join!` and take the whole app down — log,
                // back off, and retry (mirrors the backup loop's self-healing).
                error!("Certificate refresh failed, retrying in {RETRY_BACKOFF:?}: {e:?}");
                sleep(RETRY_BACKOFF).await;
            }
        }
    }

    /// One cert-refresh cycle. Sleeps `CERT_REFRESH` on the steady-state paths
    /// (cert unchanged, or freshly broadcast); returns immediately after ordering
    /// a new cert so the next iteration loads + broadcasts it. Returns `Err` on
    /// any failure — the caller logs + backs off rather than crashing.
    async fn refresh_once(
        &self,
        current_certificate: &mut Option<CertificateLoader>,
        tls_config_sender: &Sender<RustlsConfig>,
    ) -> Result<()> {
        info!("Loading certificates");
        let new_cert =
            CertificateLoader::maybe_load(&self.pool, self.settings.domain.clone()).await?;
        if *current_certificate == new_cert && current_certificate.is_some() {
            debug!("No certificate change, sleeping");
            sleep(CERT_REFRESH).await;
            return Ok(());
        }
        *current_certificate = new_cert;

        if let Some(cl) = current_certificate {
            let server_config = Arc::new(
                ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(cl.certificate_chain.clone(), cl.private_key.clone_key())?,
            );
            let rusttls_cfg = RustlsConfig::from_config(server_config);

            info!("Sending Rustls config");
            tls_config_sender.send(rusttls_cfg.clone())?;

            //Sleeping to await the next refresh
            sleep(CERT_REFRESH).await;
        } else {
            info!("Ordering a certificate for {}", self.settings.domain);

            debug!("Getting account");
            let instant_acme = InstantAcmeDomain::create_or_load(
                &self.pool,
                self.settings.domain.clone(),
                self.client.clone(),
            )
            .await?;

            debug!("Submitting order");
            let (cert_chain, private_key) = instant_acme.order_cert().await?;

            debug!("Saving new cert");
            let cd = CertificateDao {
                domain: self.settings.domain.clone(),
                certificate_chain: cert_chain,
                private_key,
            };
            cd.save(&self.pool).await?;
        }
        Ok(())
    }
}
