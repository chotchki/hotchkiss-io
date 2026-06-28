use super::dns::cloudflare_client::CloudflareClient;
use crate::settings::Settings;
use crate::{
    coordinator::acme::{certificate_loader::CertificateLoader, instant_acme::InstantAcmeDomain},
    db::dao::certificate::CertificateDao,
};
use anyhow::Result;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};
use tokio::{sync::broadcast::Sender, time::sleep};
use tracing::{debug, error, info};

const CERT_REFRESH: Duration = Duration::new(6 * 60 * 60, 0);
/// Conservative retry on failure: 20 min ≈ 3 attempts/hour, comfortably under
/// Let's Encrypt's failed-validation rate limit (~5/hour per account+host) with
/// headroom for the occasional restart-triggered immediate retry — so a stuck
/// order can't crash-loop the app into a rate-limit ban.
const RETRY_BACKOFF: Duration = Duration::new(20 * 60, 0);

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

    pub async fn start(&self, tls_config_sender: Sender<Arc<ServerConfig>>) -> Result<()> {
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
        tls_config_sender: &Sender<Arc<ServerConfig>>,
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
            let server_config = Arc::new(build_server_config(
                cl.certificate_chain.clone(),
                cl.private_key.clone_key(),
            )?);

            // Broadcast the renewable ServerConfig; the endpoints service owns
            // the live RustlsConfig handle and reloads it (so a renewed cert is
            // applied to the running server without a restart).
            info!("Broadcasting refreshed TLS server config");
            tls_config_sender.send(server_config)?;

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

/// Build the HTTPS `ServerConfig` for a loaded cert, advertising HTTP/2 (then
/// HTTP/1.1) via ALPN. Factored out so the ALPN policy is unit-testable:
/// `axum-server` serves h2 over hyper, but WITHOUT this ALPN nothing selects it
/// and the server silently stays HTTP/1.1 (the pre-v0.0.69 behavior — every
/// page's many small vendored assets serialized over the 6-connection cap).
fn build_server_config(
    certificate_chain: Vec<CertificateDer<'static>>,
    private_key: PrivateKeyDer<'static>,
) -> Result<ServerConfig> {
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificate_chain, private_key)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::build_server_config;
    use rcgen::{CertificateParams, DistinguishedName, KeyPair};
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

    /// Regression guard for the HTTP/2 win (v0.0.69): the served TLS config must
    /// advertise `h2` first, then `http/1.1`. Drop or reorder the ALPN and
    /// axum-server silently falls back to HTTP/1.1 — this catches that.
    #[test]
    fn server_config_advertises_h2_then_http11() {
        // A dev-dependency pulls in `ring` alongside the app's `aws-lc-rs`, so the
        // test profile can't auto-pick a rustls provider (the release binary has
        // only aws-lc-rs). Install it explicitly; idempotent across tests.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let key_pair = KeyPair::generate().expect("keypair");
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).expect("params");
        params.distinguished_name = DistinguishedName::new();
        let cert = params.self_signed(&key_pair).expect("self-signed cert");

        let chain = vec![cert.der().clone()];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

        let config = build_server_config(chain, key).expect("server config");
        assert_eq!(
            config.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            "HTTPS must offer HTTP/2 first with an HTTP/1.1 fallback"
        );
    }
}
