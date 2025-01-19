use super::dns::cloudflare_client::CloudflareClient;
use crate::{
    coordinator::acme::{certificate_loader::CertificateLoader, instant_acme::InstantAcmeDomain},
    db::dao::certificate::{self, CertificateDao},
};
use anyhow::Result;
use axum_server::tls_rustls::RustlsConfig;
use hickory_resolver::TokioAsyncResolver;
use rustls::ServerConfig;
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};
use tokio::{sync::broadcast::Sender, time::sleep};
use tracing::{debug, info};

const CERT_REFRESH: Duration = Duration::new(6 * 60 * 60, 0);

#[derive(Debug)]
pub struct AcmeProviderService {
    pool: SqlitePool,
    domain: String,
}

impl AcmeProviderService {
    pub fn create(pool: SqlitePool, domain: String) -> Result<Self> {
        Ok(Self { pool, domain })
    }

    pub async fn start(
        &self,
        tls_config_sender: Sender<RustlsConfig>,
        token: String,
        resolver: TokioAsyncResolver,
    ) -> Result<()> {
        let mut current_certificate: Option<CertificateLoader> = None;
        loop {
            info!("Loading certificates");
            let new_cert = CertificateLoader::maybe_load(&self.pool, self.domain.clone()).await?;
            if current_certificate == new_cert && current_certificate.is_some() {
                debug!("No certificate change, sleeping");
                sleep(CERT_REFRESH).await;
                continue;
            } else {
                current_certificate = new_cert;
            }

            if let Some(cl) = &current_certificate {
                let server_config = Arc::new(
                    ServerConfig::builder()
                        .with_no_client_auth()
                        .with_single_cert(
                            cl.certificate_chain.clone(),
                            cl.private_key.clone_key(),
                        )?,
                );
                let rusttls_cfg = RustlsConfig::from_config(server_config);

                info!("Sending Rustls config");
                tls_config_sender.send(rusttls_cfg.clone())?;

                //Sleeping to await the next refresh
                sleep(CERT_REFRESH).await;
            } else {
                info!("Ordering a certificate for  {}", self.domain);
                let client = CloudflareClient::new(token.clone(), self.domain.clone())?;

                debug!("Getting account");
                let instant_acme =
                    InstantAcmeDomain::create_or_load(&self.pool, self.domain.clone(), client)
                        .await?;

                debug!("Submitting order");
                let (cert_chain, private_key) = instant_acme
                    .order_cert(self.domain.clone(), &resolver)
                    .await?;

                debug!("Saving new cert");
                certificate::upsert(
                    &self.pool,
                    &CertificateDao {
                        domain: self.domain.clone(),
                        certificate_chain: cert_chain,
                        private_key,
                    },
                )
                .await?;
            }
        }
    }
}
