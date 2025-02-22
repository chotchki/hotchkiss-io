use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use sqlx::{query, query_as, SqlitePool};
use tracing::debug;

#[derive(Clone, Debug, Deserialize, FromRow, Serialize, PartialEq)]
pub struct AcmeAccountDao {
    pub domain: String,
    pub account_credentials: String, //Serialized credentials
}

impl AcmeAccountDao {
    pub async fn create(&self, pool: &SqlitePool) -> Result<()> {
        query!(
            r#"
        insert into instant_acme_domains (
            domain,
            account_credentials
        ) VALUES (
            ?1,
            ?2
        )
        "#,
            self.domain,
            self.account_credentials
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn find_by_domain(pool: &SqlitePool, domain: &str) -> Result<Option<AcmeAccountDao>> {
        debug!("Preparing query");
        let iad: Option<AcmeAccountDao> = query_as!(
            AcmeAccountDao,
            r#"
            select 
                domain,
                account_credentials
            from 
                instant_acme_domains
            where domain = ?1
        "#,
            domain
        )
        .fetch_optional(pool)
        .await?;

        debug!("Query finished");

        Ok(iad)
    }

    pub async fn update(&self, pool: &SqlitePool) -> Result<()> {
        query!(
            r#"
        update instant_acme_domains
        set
            domain = ?1,
            account_credentials = ?2
        where domain = ?1
        "#,
            self.domain,
            self.account_credentials
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn roundtrip(pool: SqlitePool) -> Result<()> {
        let ad = AcmeAccountDao {
            domain: "example.com".to_string(),
            account_credentials: "stuff!".to_string(),
        };

        ad.create(&pool).await?;

        let found_ad = AcmeAccountDao::find_by_domain(&pool, "example.com")
            .await?
            .expect("We just inserted this value");

        assert_eq!(ad, found_ad);

        Ok(())
    }
}
