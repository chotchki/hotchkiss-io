use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{prelude::FromRow, sqlite::SqliteRow};
use sqlx::{query, query_as, Row, SqlitePool};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CertificateDao {
    pub domain: String,
    pub certificate_chain: String, //Just a PEM
    pub private_key: String,       //Just a PEM
}

impl FromRow<'_, SqliteRow> for CertificateDao {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        Ok(CertificateDao {
            domain: row.try_get("domain")?,
            certificate_chain: row.try_get("certificate_chain")?,
            private_key: row.try_get("private_key")?,
        })
    }
}

pub async fn upsert(pool: &SqlitePool, cert_dao: &CertificateDao) -> Result<()> {
    query!(
        r#"
        INSERT INTO certificates (
            domain,
            certificate_chain,
            private_key
        ) VALUES (
            ?1,
            ?2,
            ?3
        ) 
        ON CONFLICT(domain) 
        DO UPDATE 
            SET certificate_chain = ?2,
                private_key = ?3
        "#,
        cert_dao.domain,
        cert_dao.certificate_chain,
        cert_dao.private_key
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn find_by_domain(pool: &SqlitePool, domain: &str) -> Result<Option<CertificateDao>> {
    let iad: Option<CertificateDao> = query_as!(
        CertificateDao,
        r#"
            select 
                domain,
                certificate_chain,
                private_key
            from 
                certificates
            where domain = ?1
        "#,
        domain
    )
    .fetch_optional(pool)
    .await?;

    Ok(iad)
}
