use anyhow::{bail, Result};
use instant_acme::{
    Account, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt, NewAccount, NewOrder,
    OrderStatus,
};
use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info};

use crate::{
    coordinator::dns::cloudflare_client::CloudflareClient, db::dao::acme_account::AcmeAccountDao,
};

pub struct InstantAcmeDomain {
    domain: String,
    account: Account,
    client: CloudflareClient,
}

impl InstantAcmeDomain {
    pub async fn create_or_load(
        pool: &SqlitePool,
        domain: String,
        client: CloudflareClient,
    ) -> Result<InstantAcmeDomain> {
        match AcmeAccountDao::find_by_domain(pool, &domain).await? {
            Some(aa) => {
                debug!("Found account");
                let account_credentials = serde_json::from_str(&aa.account_credentials)?;
                let account = Account::from_credentials(account_credentials).await?;
                Ok(Self {
                    domain,
                    account,
                    client,
                })
            }
            None => {
                debug!("Making a new account");
                let lets_encrypt: &str = if cfg!(debug_assertions) {
                    debug!("Using Let's Encrypt Staging");
                    LetsEncrypt::Staging.url()
                } else {
                    debug!("Using Let's Encrypt Prod");
                    LetsEncrypt::Production.url()
                };
                let (account, credentials) = Account::create(
                    &NewAccount {
                        contact: &[],
                        terms_of_service_agreed: true,
                        only_return_existing: false,
                    },
                    lets_encrypt,
                    None,
                )
                .await?;

                debug!("Saving the new credentials");
                let aa = AcmeAccountDao {
                    domain: domain.clone(),
                    account_credentials: serde_json::to_string(&credentials)?,
                };
                aa.create(pool).await?;

                debug!("Returning credentials");
                Ok(Self {
                    domain,
                    account,
                    client,
                })
            }
        }
    }

    pub async fn order_cert(&self) -> Result<(String, String)> {
        info!("Ordering a new certificate for {}", self.domain);

        debug!("Clearing old proofs");
        self.client
            .clean_proof(&Self::create_proof_domain(&self.domain))
            .await?;

        let identifier = Identifier::Dns(self.domain.clone());

        debug!("Creating a new order");
        let mut order = self
            .account
            .new_order(&NewOrder {
                identifiers: &[identifier],
            })
            .await?;

        debug!("Getting order authorizations");
        let authorizations = order.authorizations().await?;
        let mut challenges = vec![];
        for authz in &authorizations {
            match authz.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                _ => todo!(),
            }

            let challenge = authz
                .challenges
                .iter()
                .find(|c| c.r#type == ChallengeType::Dns01)
                .ok_or_else(|| anyhow::anyhow!("no dns01 challenge found"))?;

            let Identifier::Dns(identifier) = &authz.identifier;

            debug!("Setting dns proof");
            self.client
                .create_proof(
                    &Self::create_proof_domain(&self.domain),
                    &order.key_authorization(challenge).dns_value(),
                )
                .await?;

            challenges.push((identifier, &challenge.url));
        }

        for (_, url) in &challenges {
            order.set_challenge_ready(url).await?;
        }

        // Exponentially back off until the order becomes ready or invalid.
        let mut tries = 1u8;
        let mut delay = Duration::from_millis(250);
        loop {
            sleep(delay).await;
            let state = order.refresh().await?;
            if let OrderStatus::Ready | OrderStatus::Invalid = state.status {
                info!("order state: {:#?}", state);
                break;
            }

            delay *= 2;
            tries += 1;
            match tries < 5 {
                true => info!(?state, tries, "order is not ready, waiting {delay:?}"),
                false => {
                    error!(tries, "order is not ready: {state:#?}");
                    return Err(anyhow::anyhow!("order is not ready"));
                }
            }
        }

        let state = order.state();
        if state.status != OrderStatus::Ready {
            bail!("unexpected order status: {:?}", state.status);
        }

        // If the order is ready, we can provision the certificate.
        // Use the rcgen library to create a Certificate Signing Request.
        let mut params = CertificateParams::new(vec![self.domain.clone()])?;
        params.distinguished_name = DistinguishedName::new();
        let private_key = KeyPair::generate()?;
        let csr = params.serialize_request(&private_key)?;

        order.finalize(csr.der()).await?;
        let cert_chain_pem = loop {
            match order.certificate().await? {
                Some(cert_chain_pem) => break cert_chain_pem,
                None => sleep(Duration::from_secs(1)).await,
            }
        };

        Ok((cert_chain_pem, private_key.serialize_pem()))
    }

    fn create_proof_domain(domain: &str) -> String {
        format!("_acme-challenge.{domain}")
    }
}
