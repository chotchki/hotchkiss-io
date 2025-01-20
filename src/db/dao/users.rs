use super::roles::Roles;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{query, query_as, sqlite::SqliteRow, Error::ColumnDecode, FromRow, Row, SqlitePool};
use std::str::FromStr;
use uuid::Uuid;
use webauthn_rs::prelude::Passkey;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct User {
    pub display_name: String,
    pub id: Uuid,
    pub keys: Vec<Passkey>,
    pub role: Roles,
}

impl FromRow<'_, SqliteRow> for User {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        Ok(Self {
            display_name: row.try_get("display_name")?,
            id: Uuid::parse_str(row.try_get("id")?).map_err(|e| ColumnDecode {
                index: "id".to_string(),
                source: Box::new(e),
            })?,
            keys: serde_json::from_str(row.try_get("keys")?).map_err(|e| ColumnDecode {
                index: "keys".to_string(),
                source: Box::new(e),
            })?,
            role: Roles::from_str(row.try_get("role")?).map_err(|e| ColumnDecode {
                index: "role".to_string(),
                source: Box::new(e),
            })?,
        })
    }
}

pub async fn create(pool: &SqlitePool, user: &mut User) -> Result<()> {
    let id = user.id.to_string();
    let keys = serde_json::to_string(&user.keys)?;

    //Handling the fist user
    let rec = query!(
        r#"
        insert into users (
            display_name,
            id,
            keys,
            app_role
        ) VALUES (
            ?1,
            ?2,
            ?3,
            CASE WHEN (SELECT COUNT(*) from users) == 0
            THEN 'Admin'
            ELSE 'Registered'
            END
        )
        RETURNING app_role
        "#,
        user.display_name,
        id,
        keys
    )
    .fetch_one(pool)
    .await?;

    user.role = Roles::from_str(&rec.app_role)?;

    Ok(())
}

pub async fn find_by_passkey(pool: &SqlitePool, passkey: Passkey) -> Result<Option<User>> {
    Ok(query_as(
        r#"
        SELECT 
            display_name,
            id,
            keys,
            app_role as role
        FROM users
        WHERE
            EXISTS (SELECT 1 FROM json_each(keys) WHERE value = ?1)
    "#,
    )
    .bind(serde_json::to_string(&passkey)?)
    .fetch_optional(pool)
    .await?)
}
