//! Boots just the web layer (no DNS/ACME/IP coordinator) against a throwaway
//! SQLite, on plain HTTP on an ephemeral port bound to all interfaces — for
//! integration tests, local poking, and iOS Simulator e2e (which needs to
//! reach the host's LAN IP, not just loopback). Lives in the lib (not
//! `tests/`) so it can reach the crate-internal `create_router` / `AppState`
//! / `DatabaseHandle` without making half the crate `pub`.

use std::net::SocketAddr;

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::{net::TcpListener, task::JoinHandle};
use tower_sessions_sqlx_store::SqliteStore;
use url::Url;
use uuid::Uuid;
use webauthn_rs::WebauthnBuilder;

use crate::{
    db::{
        dao::{api_keys::ApiKeyDao, content_pages::ContentPageDao, roles::Role, users::UserDao},
        database_handle::DatabaseHandle,
    },
    media::MediaStore,
    web::{app_state::AppState, router::create_router},
};

/// A running test instance. `Drop` aborts the server task and deletes the temp DB.
pub struct TestServer {
    /// e.g. `http://localhost:54321` (no trailing slash). Hit it via `localhost`,
    /// not `127.0.0.1` — the WebAuthn rp_origin is `http://localhost:<port>`.
    pub base_url: String,
    pub port: u16,
    pub pool: SqlitePool,
    server: JoinHandle<()>,
    _db: TempDb,
}

impl TestServer {
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// URL reachable from off-host clients (iOS Simulator, phones on the LAN, etc).
    /// Resolves the Mac's primary LAN IP at call time. Returns an error if there
    /// isn't one (e.g. wifi off, no networks). WebAuthn won't work via this URL —
    /// rp_origin is `localhost:<port>`. Use it for non-auth UI checks.
    pub fn lan_url(&self, path: &str) -> Result<String> {
        let ip = local_ip_address::local_ip()
            .map_err(|e| anyhow::anyhow!("could not resolve host LAN IP: {e}"))?;
        Ok(format!("http://{ip}:{port}{path}", ip = ip, port = self.port))
    }

    /// Seed an Admin user and mint an API key for it; returns the plaintext key.
    /// The first user in a fresh DB is auto-promoted to Admin, so the key delegates
    /// Admin — for exercising `Authorization: Bearer hio_…` auth.
    pub async fn seed_admin_api_key(&self, label: &str) -> Result<String> {
        let mut user = UserDao {
            display_name: "api-tester".to_string(),
            id: Uuid::now_v7(),
            keys: sqlx::types::Json(vec![]),
            role: Role::Registered,
        };
        user.create(&self.pool).await?; // first user → Admin (enforced in create)
        let (key, _) = ApiKeyDao::create(&self.pool, &user.id, label).await?;
        Ok(key)
    }

    /// Seed a top-level content page. Returns the created page so the caller can
    /// use its id. (A fresh migrated DB already has the seeded `login`/`projects`
    /// special pages, so `/` redirects even before this is called.)
    pub async fn seed_content_page(&self, name: &str, markdown: &str) -> Result<ContentPageDao> {
        ContentPageDao::create(
            &self.pool,
            None,
            name.to_string(),
            None,
            markdown.to_string(),
            None,
        )
        .await
    }

    /// Seed a child of the `blog` special_page (seeded by migration 0010).
    pub async fn seed_blog_post(&self, slug: &str, markdown: &str) -> Result<()> {
        let blog = ContentPageDao::find_by_name(&self.pool, None, "blog")
            .await?
            .ok_or_else(|| anyhow::anyhow!("blog special_page missing — migration 0010?"))?;
        ContentPageDao::create(
            &self.pool,
            Some(blog.page_id),
            slug.to_string(),
            None,
            markdown.to_string(),
            None,
        )
        .await?;
        Ok(())
    }

    /// Seed a child of the `resume` special_page (seeded by migration 0012) — the
    /// résumé content. `/resume` renders the newest child.
    pub async fn seed_resume(&self, markdown: &str) -> Result<()> {
        let resume = ContentPageDao::find_by_name(&self.pool, None, "resume")
            .await?
            .ok_or_else(|| anyhow::anyhow!("resume special_page missing — migration 0012?"))?;
        ContentPageDao::create(
            &self.pool,
            Some(resume.page_id),
            "main".to_string(),
            None,
            markdown.to_string(),
            None,
        )
        .await?;
        Ok(())
    }

    /// Seed a user with an explicit role ("Admin" | "Registered"), returning their
    /// id — for the `/admin/users` management tests. Bypasses the first-user→Admin
    /// rule so a test can stand up several users deterministically.
    pub async fn seed_user(&self, display_name: &str, role: &str) -> Result<Uuid> {
        let id = Uuid::now_v7();
        let id_str = id.to_string();
        sqlx::query("INSERT INTO users (display_name, id, keys, app_role) VALUES (?1, ?2, '[]', ?3)")
            .bind(display_name)
            .bind(id_str)
            .bind(role)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    /// Seed a child of the `projects` special_page (seeded by migration 0007).
    pub async fn seed_project(&self, slug: &str, markdown: &str) -> Result<()> {
        let projects = ContentPageDao::find_by_name(&self.pool, None, "projects")
            .await?
            .ok_or_else(|| anyhow::anyhow!("projects special_page missing — migration 0007?"))?;
        ContentPageDao::create(
            &self.pool,
            Some(projects.page_id),
            slug.to_string(),
            None,
            markdown.to_string(),
            None,
        )
        .await?;
        Ok(())
    }

    /// Seed a child of the `3d` special_page (seeded by migration 0023) — a
    /// gallery model page.
    pub async fn seed_3d_model(&self, slug: &str, markdown: &str) -> Result<()> {
        let three_d = ContentPageDao::find_by_name(&self.pool, None, "3d")
            .await?
            .ok_or_else(|| anyhow::anyhow!("3d special_page missing — migration 0023?"))?;
        ContentPageDao::create(
            &self.pool,
            Some(three_d.page_id),
            slug.to_string(),
            None,
            markdown.to_string(),
            None,
        )
        .await?;
        Ok(())
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.server.abort();
        // `_db` drops after this and removes the file(s).
    }
}

struct TempDb(std::path::PathBuf);

impl Drop for TempDb {
    fn drop(&mut self) {
        let p = self.0.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(format!("{p}-wal"));
        let _ = std::fs::remove_file(format!("{p}-shm"));
    }
}

/// Boot the web layer on a random port (all interfaces) against a fresh
/// migrated SQLite. Binds `0.0.0.0` rather than `127.0.0.1` so iOS Simulator
/// (which has its own loopback) can reach the host's LAN IP — `localhost`
/// callers like the reqwest/chromiumoxide tests still resolve correctly via
/// `lan_url` is just an additional option, not a replacement.
pub async fn spawn_test_server() -> Result<TestServer> {
    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    let origin = Url::parse(&format!("http://localhost:{port}/"))?;
    let webauthn = WebauthnBuilder::new("localhost", &origin)?.build()?;

    let db_path = std::env::temp_dir().join(format!("hotchkiss-test-{}.sqlite", Uuid::new_v4()));
    let pool = DatabaseHandle::create(&db_path).await?;

    let session_store = SqliteStore::new(pool.clone());
    session_store.migrate().await?;

    let app_state = AppState {
        pool: pool.clone(),
        session_store,
        webauthn,
        site_host: "hotchkiss.io".to_string(),
        log_path: std::env::temp_dir().join(format!("hotchkiss-test-logs-{}", Uuid::new_v4())),
        media_store: MediaStore::new(
            vec![std::env::temp_dir().join(format!("hotchkiss-test-media-{}", Uuid::new_v4()))],
            0,
        ),
    };
    let router = create_router(app_state).await?;

    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    Ok(TestServer {
        base_url: format!("http://localhost:{port}"),
        port,
        pool,
        server,
        _db: TempDb(db_path),
    })
}
