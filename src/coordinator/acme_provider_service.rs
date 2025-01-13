use acme_lib::{create_p256_key, Certificate, Directory, DirectoryUrl};
use anyhow::{bail, Result};
use axum_server::tls_rustls::RustlsConfig;
use hickory_resolver::{
    proto::rr::{RData, RecordType},
    TokioAsyncResolver,
};
use rustls::{
    pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer},
    ServerConfig,
};
use sqlx::SqlitePool;
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::{runtime::Handle, sync::broadcast::Sender, time::sleep};
use tracing::{debug, info, instrument};

use super::{acme::acme_persist_key::AcmePersistKey, dns::cloudflare_client::CloudflareClient};

static BASE_URL: LazyLock<DirectoryUrl> = LazyLock::new(|| {
    if cfg!(debug_assertions) {
        DirectoryUrl::LetsEncryptStaging
    } else {
        DirectoryUrl::LetsEncrypt
    }
});

const CERT_EMAIL: &str = "foo@bar.com";
const CERT_REFRESH: Duration = Duration::new(6 * 60 * 60, 0);

#[derive(Debug)]
pub struct AcmeProviderService {
    handle: Arc<Handle>,
    persist: AcmePersistKey,
    domain: String,
    client: CloudflareClient,
    resolver: TokioAsyncResolver,
}

impl AcmeProviderService {
    pub fn create(
        pool: SqlitePool,
        resolver: TokioAsyncResolver,
        token: String,
        domain: String,
    ) -> Result<Self> {
        let handle = Arc::new(Handle::current());
        Ok(Self {
            handle: handle.clone(),
            persist: AcmePersistKey::create(pool, handle),
            domain: domain.clone(),
            client: CloudflareClient::new(token, domain)?,
            resolver,
        })
    }

    pub async fn start(&self, tls_config_sender: Sender<RustlsConfig>) -> Result<()> {
        debug!("Starting acme for domain {}", self.domain);
        let acme_cert = self.get_certificate().await?;

        let mut rustls_certs: Vec<CertificateDer<'static>> = vec![];
        for cert in CertificateDer::pem_slice_iter(acme_cert.certificate().as_bytes()) {
            if let Ok(cert) = cert {
                rustls_certs.push(cert.into_owned());
            } else {
                bail!("Could not parse cert");
            }
        }

        let rustls_private_key = PrivateKeyDer::from_pem_slice(acme_cert.private_key().as_bytes())?;

        let server_config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(rustls_certs, rustls_private_key)?,
        );
        let rusttls_cfg = RustlsConfig::from_config(server_config);

        info!("Sending initial Rustls config");
        tls_config_sender.send(rusttls_cfg.clone())?;

        loop {
            tokio::time::sleep(CERT_REFRESH).await;

            let acme_cert = self.get_certificate().await?;

            let rustls_certs = vec![acme_cert.certificate_der()];

            rusttls_cfg
                .reload_from_der(rustls_certs, acme_cert.private_key_der())
                .await?;
        }
    }

    #[instrument]
    pub async fn get_certificate(&self) -> Result<Certificate> {
        debug!("Creating directory");
        let dir = Directory::from_url(self.persist.clone(), BASE_URL.clone())?;

        debug!("Looking up account");
        let acc = self
            .handle
            .spawn_blocking(move || dir.account(CERT_EMAIL))
            .await??;

        debug!("Accessing pre-existing cert");
        let acc2 = acc.clone();
        let domain2 = self.domain.clone();
        let maybe_cert = self
            .handle
            .spawn_blocking(move || acc2.certificate(&domain2))
            .await??;

        if let Some(cert) = maybe_cert {
            if cert.valid_days_left() > 30 {
                debug!("Certificate is valid for more than 30 days");
                return Ok(cert);
            }
        }

        debug!("Ordering new certificate");
        let acc3 = acc.clone();
        let domain3 = self.domain.clone();
        let mut ord_new = self
            .handle
            .spawn_blocking(move || acc3.new_order(&domain3, &[]))
            .await??;

        let ord_csr = loop {
            if let Some(ord_csr) = ord_new.confirm_validations() {
                break ord_csr;
            }

            let (auths, ord_new2) = self
                .handle
                .spawn_blocking(move || (ord_new.authorizations(), ord_new))
                .await?;
            let chall = auths?[0].dns_challenge();
            ord_new = ord_new2;

            self.client
                .create_proof(&Self::create_proof_domain(&self.domain), &chall.dns_proof())
                .await?;

            //Let's make sure we can see the new proof before we call to refresh ACME
            self.wait_for_propogation(chall.dns_proof()).await?;

            self.handle
                .spawn_blocking(|| chall.validate(1000))
                .await??;
            let (_, ord_new3) = self
                .handle
                .spawn_blocking(move || (ord_new.refresh(), ord_new))
                .await?;
            ord_new = ord_new3;
        };

        let pkey_pri = create_p256_key();
        let ord_cert = ord_csr.finalize_pkey(pkey_pri, 5000)?;

        debug!("Downloading new ceritifcate");
        let cert = self
            .handle
            .spawn_blocking(move || ord_cert.download_and_save_cert())
            .await??;

        debug!("Certificate is good");
        Ok(cert)
    }

    async fn wait_for_propogation(&self, challenge: String) -> Result<()> {
        let domain_proof_str = Self::create_proof_domain(&self.domain);
        let box_challenge: Box<[u8]> = challenge.as_bytes().into();
        loop {
            let proof_value = match self
                .resolver
                .lookup(domain_proof_str.clone() + ".", RecordType::TXT)
                .await
            {
                Ok(l) => l
                    .into_iter()
                    .filter_map(|r| match r {
                        RData::TXT(t) => Some(t),
                        _ => None,
                    })
                    .flat_map(|x| x.txt_data().to_owned())
                    .any(|x| x == box_challenge),
                Err(e) => {
                    debug!("Resolver Error looking for proof of {}", e);
                    false
                }
            };

            if proof_value {
                return Ok(());
            }

            sleep(Duration::from_secs(60)).await;
            tracing::debug!(
                "Domain {} with value {} not found",
                domain_proof_str,
                challenge
            );
        }
    }

    fn create_proof_domain(domain: &str) -> String {
        format!("_acme-challenge.{domain}")
    }
}
