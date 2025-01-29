use anyhow::Result;
use serde_json::json;
use sqlx::Error::ColumnDecode;
use sqlx::{prelude::FromRow, query, query_as, sqlite::SqliteRow, Row, SqlitePool};
use tower_sessions::cookie::Key;

#[derive(Clone, Debug)]
pub struct CryptoKey {
    pub id: i64,
    pub key_value: Key,
}

impl FromRow<'_, SqliteRow> for CryptoKey {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        let key_value: &[u8] =
            serde_json::from_str(row.try_get("key_value")?).map_err(|e| ColumnDecode {
                index: "key_value".to_string(),
                source: Box::new(e),
            })?;
        Ok(CryptoKey {
            id: row.try_get("id")?,
            key_value: Key::try_from(key_value).map_err(|e| ColumnDecode {
                index: "key_value".to_string(),
                source: Box::new(e),
            })?,
        })
    }
}

pub async fn create(pool: &SqlitePool, key: &CryptoKey) -> Result<()> {
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

    Ok(key)
}

pub async fn get_or_create(pool: &SqlitePool, id: i64) -> Result<CryptoKey> {
    match find_by_id(pool, id).await? {
        Some(s) => Ok(s),
        None => {
            let key = CryptoKey {
                id,
                key_value: Key::generate(),
            };
            create(pool, &key).await?;
            Ok(key)
        }
    }
}
