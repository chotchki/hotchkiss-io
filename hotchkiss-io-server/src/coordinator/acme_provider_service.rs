use acme_lib::{create_p256_key, Certificate, Directory, DirectoryUrl};
use anyhow::Result;
use axum_server::tls_rustls::RustlsConfig;
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer},
    ServerConfig,
};
use sqlx::SqlitePool;
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::{process::Command, runtime::Handle, sync::broadcast::Sender, time::sleep};

use crate::certificate::AcmePersistKey;

use super::dns::cloudflare_client::CloudflareClient;

static BASE_URL: LazyLock<DirectoryUrl> = LazyLock::new(|| {
    if cfg!(debug_assertions) {
        DirectoryUrl::LetsEncryptStaging
    } else {
        DirectoryUrl::LetsEncrypt
    }
});

const CERT_EMAIL: &str = "foo@bar.com";
const CERT_REFRESH: Duration = Duration::new(6 * 60 * 60, 0);

pub struct AcmeProviderService {
    handle: Arc<Handle>,
    persist: AcmePersistKey,
    domain: String,
    client: CloudflareClient,
}

impl AcmeProviderService {
    pub fn create(pool: SqlitePool, token: String, domain: String) -> Result<Self> {
        let handle = Arc::new(Handle::current());
        Ok(Self {
            handle: handle.clone(),
            persist: AcmePersistKey::create(pool, handle),
            domain: domain.clone(),
            client: CloudflareClient::new(token, domain)?,
        })
    }

    pub async fn start(&self, tls_config_sender: Sender<RustlsConfig>) -> Result<()> {
        let acme_cert = self.get_certificate().await?;

        let rustls_certs =
            vec![CertificateDer::from_slice(&acme_cert.certificate_der()).into_owned()];
        let rustls_private_key = PrivatePkcs1KeyDer::from(acme_cert.private_key_der());

        let server_config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(rustls_certs, PrivateKeyDer::Pkcs1(rustls_private_key))?,
        );
        let rusttls_cfg = RustlsConfig::from_config(server_config);

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

    pub async fn get_certificate(&self) -> Result<Certificate> {
        let dir = Directory::from_url(self.persist.clone(), BASE_URL.clone())?;

        let acc = self
            .handle
            .spawn_blocking(move || dir.account(CERT_EMAIL))
            .await??;

        let acc2 = acc.clone();
        let domain2 = self.domain.clone();
        let maybe_cert = self
            .handle
            .spawn_blocking(move || acc2.certificate(&domain2))
            .await??;

        if let Some(cert) = maybe_cert {
            if cert.valid_days_left() > 30 {
                return Ok(cert);
            }
        }

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
        let cert = ord_cert.download_and_save_cert()?;

        Ok(cert)
    }

    async fn wait_for_propogation(&self, challenge: String) -> Result<()> {
        let domain_proof_str = Self::create_proof_domain(&self.domain);
        loop {
            let output = Command::new("dig")
                .arg(domain_proof_str.clone())
                .arg("TXT")
                .output()
                .await?;
            let output_str = String::from_utf8(output.stdout)?;
            for line in output_str.lines() {
                if line.starts_with(&domain_proof_str) {
                    let line_parts: Vec<&str> = line.split_terminator('"').collect();
                    if let Some(found) = line_parts.get(1) {
                        if *found == challenge {
                            return Ok(());
                        }
                    }
                }
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
