use crate::coordinator::backup;
use crate::db::dao::request_log::RequestLogDao;
use crate::settings::Settings;
use crate::web::{app_state::AppState, router::create_router};
use anyhow::{Context, Result, bail};
use axum::{
    BoxError,
    handler::HandlerWithoutStateExt,
    http::{Uri, uri::Authority},
    response::Redirect,
};
use axum_extra::extract::Host;
use axum_server::tls_rustls::RustlsConfig;
use reqwest::StatusCode;
use sqlx::SqlitePool;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};
use tokio::{sync::broadcast::Receiver, task::JoinSet};
use tower_sessions::ExpiredDeletion;
use tower_sessions_sqlx_store::SqliteStore;
use tracing::debug;
use url::Url;
use webauthn_rs::{Webauthn, WebauthnBuilder};

pub struct EndpointsProviderService {
    pool: SqlitePool,
    session_store: SqliteStore,
    webauthn: Webauthn,
    http_port: u16,
    https_port: u16,
    backup_path: PathBuf,
}

impl EndpointsProviderService {
    pub async fn create(settings: Arc<Settings>, pool: SqlitePool) -> Result<Self> {
        let session_store = SqliteStore::new(pool.clone());
        session_store.migrate().await?;

        // The rp_origin must match the browser's origin exactly, *including* a
        // non-default port: prod serves :443 (port omitted), but beta serves
        // :8443, and without the port WebAuthn rejects every ceremony with
        // "relying party origin does not match our servers information".
        let origin_str = if settings.https_port == 443 {
            format!("https://{}/", settings.domain)
        } else {
            format!("https://{}:{}/", settings.domain, settings.https_port)
        };
        let origin = Url::parse(&origin_str).context("Parsing the rp_origin")?;

        // rp_id defaults to the served domain but can be a registrable parent
        // (beta sets `hotchkiss.io` so prod passkeys authenticate against beta).
        let webauthn = WebauthnBuilder::new(&settings.webauthn_rp_id, &origin)?.build()?;

        Ok(Self {
            pool,
            session_store,
            webauthn,
            http_port: settings.http_port,
            https_port: settings.https_port,
            backup_path: settings.backup_path.clone(),
        })
    }

    pub async fn start(&self, mut tls_config_reciever: Receiver<RustlsConfig>) -> Result<()> {
        let config = tls_config_reciever.recv().await?;

        let app_state = AppState {
            session_store: self.session_store.clone(),
            pool: self.pool.clone(),
            webauthn: self.webauthn.clone(),
        };

        let http_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), self.http_port);
        let https_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), self.https_port);

        //let http = tokio::spawn(Self::http_server(http_addr));
        //let https = tokio::spawn(Self::https_server(https_addr, app_state, config));

        //let deletion_task = tokio::task::spawn(
        //    self.session_store
        //        .clone()
        //        .continuously_delete_expired(tokio::time::Duration::from_secs(60)),
        //);

        let mut set = JoinSet::new();
        set.spawn(Self::https_server(https_addr, app_state, config));
        set.spawn(Self::http_server(http_addr, self.https_port));

        // Session GC: prune expired sessions hourly. Self-healing — a transient
        // SQLite error is logged, not propagated, so it can't fail the JoinSet
        // (and take the live HTTP servers down) over housekeeping. Mirrors the
        // prune + backup loops below.
        let session_store = self.session_store.clone();
        set.spawn(async move {
            let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(60 * 60));
            loop {
                tick.tick().await;
                if let Err(e) = session_store.delete_expired().await {
                    tracing::warn!("session GC failed: {e}");
                }
            }
        });

        // Prune request_log rows older than the retention window, daily.
        let prune_pool = self.pool.clone();
        set.spawn(async move {
            const RETAIN_DAYS: i64 = 90;
            let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(60 * 60 * 24));
            loop {
                tick.tick().await;
                match RequestLogDao::prune_before(&prune_pool, RETAIN_DAYS).await {
                    Ok(n) if n > 0 => {
                        tracing::info!("Pruned {n} request_log rows older than {RETAIN_DAYS} days")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("request_log prune failed: {e}"),
                }
            }
        });

        // Take a dated VACUUM INTO snapshot of the DB daily (first tick fires
        // immediately at startup), then prune to a rolling window. CRITICAL:
        // this loop never returns and every fallible step is matched + logged,
        // so a backup failure can't bubble out and fail the coordinator's
        // `try_join!` (which would take the whole app down). The interval simply
        // ticks again next day; a single bad day self-heals on the next run.
        let backup_pool = self.pool.clone();
        let backup_dir = self.backup_path.clone();
        set.spawn(async move {
            let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(60 * 60 * 24));
            loop {
                tick.tick().await;
                match backup::run_backup(&backup_pool, &backup_dir).await {
                    Ok(dest) => {
                        tracing::info!("Wrote database backup to {}", dest.display());
                        if let Err(e) =
                            backup::prune_old_backups(&backup_dir, backup::RETAIN_BACKUPS)
                        {
                            tracing::warn!("backup prune failed: {e:#}");
                        }
                    }
                    Err(e) => tracing::error!("database backup failed: {e:#}"),
                }
            }
        });

        let output = set.join_all().await;
        for o in output {
            match o {
                Ok(_) => (),
                Err(e) => {
                    bail!(e)
                }
            }
        }

        Ok(())
    }

    async fn http_server(addr: SocketAddr, https_port: u16) -> Result<()> {
        tracing::info!("HTTP Server listening on {}", addr);

        let redirect = move |Host(host): Host, uri: Uri| async move {
            match make_https(&host, uri, https_port) {
                Ok(uri) => {
                    debug!("Got connnection, redirecting");
                    Ok(Redirect::permanent(&uri.to_string()))
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to convert URI to HTTPS");
                    Err(StatusCode::BAD_REQUEST)
                }
            }
        };

        axum_server::bind(addr)
            .serve(redirect.into_make_service())
            .await?;

        Ok(())
    }

    async fn https_server(
        addr: SocketAddr,
        app_state: AppState,
        config: RustlsConfig,
    ) -> Result<()> {
        tracing::info!("HTTPS Server listening on {}", addr);

        axum_server::bind_rustls(addr, config)
            .serve(
                create_router(app_state)
                    .await?
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await?;

        Ok(())
    }
}

fn make_https(host: &str, uri: Uri, https_port: u16) -> Result<Uri, BoxError> {
    let mut parts = uri.into_parts();

    parts.scheme = Some(axum::http::uri::Scheme::HTTPS);
    if parts.path_and_query.is_none() {
        parts.path_and_query = Some("/".parse().unwrap());
    }
    let authority: Authority = host.parse()?;
    let bare_host = match authority.port() {
        Some(port_struct) => authority
            .as_str()
            .strip_suffix(port_struct.as_str())
            .unwrap()
            .strip_suffix(':')
            .unwrap(), // if authority.port() is Some(port) then we can be sure authority ends with :{port}
        None => authority.as_str(),
    };

    parts.authority = Some(format!("{bare_host}:{https_port}").parse()?);

    Ok(Uri::from_parts(parts)?)
}
