use crate::db::dao::certificate::CertificateDao;
use anyhow::Result;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use sqlx::SqlitePool;
use std::time::Duration;
use x509_parser::parse_x509_certificate;

#[derive(Debug, PartialEq, Eq)]
pub struct CertificateLoader {
    pub certificate_chain: Vec<CertificateDer<'static>>, //Just a PEM
    pub private_key: PrivateKeyDer<'static>,             //Just a PEM
}

impl CertificateLoader {
    pub async fn maybe_load(
        pool: &SqlitePool,
        domain: String,
    ) -> Result<Option<CertificateLoader>> {
        match CertificateDao::find_by_domain(pool, &domain).await? {
            Some(cd) => {
                let mut cc = vec![];
                for cert in CertificateDer::pem_slice_iter(cd.certificate_chain.as_bytes()) {
                    cc.push(cert?);
                }

                let cl = CertificateLoader {
                    certificate_chain: cc,
                    private_key: PrivateKeyDer::from_pem_slice(cd.private_key.as_bytes())?,
                };

                for cert in cl.certificate_chain.iter() {
                    let (_, x509) = parse_x509_certificate(cert)?;
                    if let Some(remaining_time) = x509.validity.time_to_expiration()
                        && remaining_time > Duration::from_secs(60 * 60 * 24 * 30)
                    {
                        continue;
                    }

                    //Cert needs renewal
                    return Ok(None);
                }

                //Certs are good to use
                Ok(Some(cl))
            }
            None => Ok(None),
        }
    }
}
