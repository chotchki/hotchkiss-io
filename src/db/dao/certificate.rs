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

impl CertificateDao {
    pub async fn save(&self, pool: &SqlitePool) -> Result<()> {
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
            self.domain,
            self.certificate_chain,
            self.private_key
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

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn save_find_and_upsert(pool: SqlitePool) -> Result<()> {
        assert!(
            CertificateDao::find_by_domain(&pool, "hotchkiss.io")
                .await?
                .is_none(),
            "no cert before save"
        );

        let cert = CertificateDao {
            domain: "hotchkiss.io".to_string(),
            certificate_chain: "CHAIN-PEM".to_string(),
            private_key: "KEY-PEM".to_string(),
        };
        cert.save(&pool).await?;

        let found = CertificateDao::find_by_domain(&pool, "hotchkiss.io")
            .await?
            .unwrap();
        assert_eq!(found.certificate_chain, "CHAIN-PEM");
        assert_eq!(found.private_key, "KEY-PEM");

        // Saving the same domain upserts (ON CONFLICT) — renewal replaces in place.
        CertificateDao {
            domain: "hotchkiss.io".to_string(),
            certificate_chain: "CHAIN-2".to_string(),
            private_key: "KEY-2".to_string(),
        }
        .save(&pool)
        .await?;
        let found = CertificateDao::find_by_domain(&pool, "hotchkiss.io")
            .await?
            .unwrap();
        assert_eq!(found.certificate_chain, "CHAIN-2");
        assert_eq!(found.private_key, "KEY-2");

        // A different domain is independent.
        assert!(
            CertificateDao::find_by_domain(&pool, "other.io")
                .await?
                .is_none()
        );

        Ok(())
    }
}
