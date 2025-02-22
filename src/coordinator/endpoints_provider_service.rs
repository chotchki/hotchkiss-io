use crate::{
    settings::Settings,
    web::{app_state::AppState, router::create_router},
};
use anyhow::{anyhow, bail, Context, Result};
use axum::{
    handler::HandlerWithoutStateExt,
    http::{uri::Authority, Uri},
    response::Redirect,
    BoxError,
};
use axum_extra::extract::Host;
use axum_server::tls_rustls::RustlsConfig;
use reqwest::StatusCode;
use sqlx::SqlitePool;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{sync::broadcast::Receiver, task::JoinSet};
use tower_sessions::ExpiredDeletion;
use tower_sessions_sqlx_store::SqliteStore;
use tracing::debug;
use url::Url;
use webauthn_rs::{Webauthn, WebauthnBuilder};

pub const HTTP_PORT: u16 = 80;
pub const HTTPS_PORT: u16 = 443;

pub struct EndpointsProviderService {
    pool: SqlitePool,
    session_store: SqliteStore,
    webauthn: Webauthn,
}

impl EndpointsProviderService {
    pub async fn create(settings: Arc<Settings>, pool: SqlitePool) -> Result<Self> {
        let session_store = SqliteStore::new(pool.clone());
        session_store.migrate().await?;

        let origin = Url::parse(&format!("https://{}/", settings.domain))
            .context("Parsing the rp_origin")?;

        let webauthn = WebauthnBuilder::new(&settings.domain, &origin)?.build()?;

        Ok(Self {
            pool,
            session_store,
            webauthn,
        })
    }

    pub async fn start(&self, mut tls_config_reciever: Receiver<RustlsConfig>) -> Result<()> {
        let config = tls_config_reciever.recv().await?;

        let app_state = AppState {
            session_store: self.session_store.clone(),
            pool: self.pool.clone(),
            webauthn: self.webauthn.clone(),
        };

        let http_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), HTTP_PORT);
        let https_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), HTTPS_PORT);

        //let http = tokio::spawn(Self::http_server(http_addr));
        //let https = tokio::spawn(Self::https_server(https_addr, app_state, config));

        //let deletion_task = tokio::task::spawn(
        //    self.session_store
        //        .clone()
        //        .continuously_delete_expired(tokio::time::Duration::from_secs(60)),
        //);

        let mut set = JoinSet::new();
        set.spawn(Self::https_server(https_addr, app_state, config));
        set.spawn(Self::http_server(http_addr));

        let session_store = self.session_store.clone();
        set.spawn(async move {
            session_store
                .continuously_delete_expired(tokio::time::Duration::from_secs(60))
                .await
                .map_err(|e| anyhow!(e))
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

    async fn http_server(addr: SocketAddr) -> Result<()> {
        tracing::info!("HTTP Server listening on {}", addr);

        let redirect = move |Host(host): Host, uri: Uri| async move {
            match make_https(&host, uri, HTTPS_PORT) {
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
            .serve(create_router(app_state).await?.into_make_service())
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
