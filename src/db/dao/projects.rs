use anyhow::Result;
use sqlx::{prelude::FromRow, query, query_as, SqliteExecutor, SqlitePool};

#[derive(Clone, Debug, FromRow, PartialEq)]
pub struct ProjectDao {
    pub project_name: String,
    pub project_category: Option<String>,
    pub project_cover_attachment: Option<String>,
}

impl ProjectDao {
    pub async fn save(&self, executor: impl SqliteExecutor<'_>) -> Result<()> {
        query!(
            r#"
        INSERT INTO project_pages (
            project_name,
            project_category,
            project_cover_attachment
        ) VALUES (
            ?1,
            ?2,
            ?3
        ) 
        ON CONFLICT(project_name) 
        DO UPDATE 
            SET project_category = ?2,
            project_cover_attachment = ?3
            
        "#,
            self.project_name,
            self.project_category,
            self.project_cover_attachment
        )
        .execute(executor)
        .await?;

        Ok(())
    }

    pub async fn delete(&self, pool: &SqlitePool) -> Result<()> {
        query!(
            r#"
            DELETE FROM project_pages
            WHERE project_name = ?1
            "#,
            self.project_name
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn get_projects_in_order(pool: &SqlitePool) -> Result<Vec<ProjectDao>> {
        let projects: Vec<ProjectDao> = query_as(
            r#"
        select 
            pp.project_name,
            pp.project_category,
            pp.project_cover_attachment
        from
            project_pages pp join content_pages cp 
            on pp.project_name = cp.page_name
        order by cp.page_order
        "#,
        )
        .fetch_all(pool)
        .await?;

        Ok(projects)
    }
}
