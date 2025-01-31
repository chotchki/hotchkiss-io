use anyhow::Result;
use serde_json::json;
use sqlx::Error::ColumnDecode;
use sqlx::{prelude::FromRow, query, query_as, sqlite::SqliteRow, Row, SqlitePool};
use tower_sessions::cookie::Key;
use tracing::debug;

#[derive(Clone, Debug, PartialEq)]
pub struct CryptoKey {
    pub id: i64,
    pub key_value: Key,
}

impl FromRow<'_, SqliteRow> for CryptoKey {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        debug!("Decoding using FromRow");

        let key_value: Vec<u8> =
            serde_json::from_str(row.try_get("key_value")?).map_err(|e| ColumnDecode {
                index: "key_value".to_string(),
                source: Box::new(e),
            })?;

        debug!("Got through serde");

        Ok(CryptoKey {
            id: row.try_get("id")?,
            key_value: Key::try_from(&key_value[..]).map_err(|e| ColumnDecode {
                index: "key_value".to_string(),
                source: Box::new(e),
            })?,
        })
    }
}

pub async fn create(pool: &SqlitePool, key: &CryptoKey) -> Result<()> {
    debug!("Creating key");

    let master_key = json!(key.key_value.master());
    query!(
        r#"
        INSERT INTO crypto_keys (
            id,
            key_value
        ) VALUES (
            ?1,
            ?2
        )
        "#,
        key.id,
        master_key
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn find_by_id(pool: &SqlitePool, id: i64) -> Result<Option<CryptoKey>> {
    debug!("Finding key id {id}");
    let key: Option<CryptoKey> = query_as(
        r#"
            select 
                id,
                key_value
            from 
                crypto_keys
            where id = ?1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    debug!("Sql returned");

    Ok(key)
}

pub async fn get_or_create(pool: &SqlitePool, id: i64) -> Result<CryptoKey> {
    match find_by_id(pool, id).await? {
        Some(s) => {
            debug!("Found key");
            Ok(s)
        }
        None => {
            debug!("Didn't find, creating");

            let key = CryptoKey {
                id,
                key_value: Key::generate(),
            };
            create(pool, &key).await?;
            Ok(key)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn roundtrip(pool: SqlitePool) -> Result<()> {
        let ck = CryptoKey {
            id: 2,
            key_value: Key::generate(),
        };

        create(&pool, &ck).await?;

        let found_ck = find_by_id(&pool, 2)
            .await?
            .expect("We just inserted this value");

        assert_eq!(ck, found_ck);

        Ok(())
    }
}
