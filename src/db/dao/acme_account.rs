use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{prelude::FromRow, sqlite::SqliteRow};
use sqlx::{query, query_as, Row, SqlitePool};
use tracing::debug;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AcmeAccountDao {
    pub domain: String,
    pub account_credentials: String, //Serialized credentials
}

impl FromRow<'_, SqliteRow> for AcmeAccountDao {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        Ok(AcmeAccountDao {
            domain: row.try_get("domain")?,
            account_credentials: row.try_get("account_credentials")?,
        })
    }
}

pub async fn create(pool: &SqlitePool, acme_domain: &AcmeAccountDao) -> Result<()> {
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
        acme_domain.domain,
        acme_domain.account_credentials
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

pub async fn update(pool: &SqlitePool, acme_domain: &AcmeAccountDao) -> Result<()> {
    query!(
        r#"
        update instant_acme_domains
        set
            domain = ?1,
            account_credentials = ?2
        where domain = ?1
        "#,
        acme_domain.domain,
        acme_domain.account_credentials
    )
    .execute(pool)
    .await?;

    Ok(())
}
