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
    db::{dao::content_pages::ContentPageDao, database_handle::DatabaseHandle},
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
