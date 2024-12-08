use crate::web::router::create_router;
use anyhow::Result;
use axum::{
    extract::Host,
    handler::HandlerWithoutStateExt,
    http::{uri::Authority, Uri},
    response::Redirect,
    BoxError,
};
use axum_server::tls_rustls::RustlsConfig;
use reqwest::StatusCode;
use sqlx::SqlitePool;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::sync::broadcast::Receiver;

pub const HTTP_PORT: u16 = 80;
pub const HTTPS_PORT: u16 = 443;

pub struct EndpointsProviderService {
    pool: SqlitePool,
}

impl EndpointsProviderService {
    pub fn create(pool: SqlitePool) -> Result<Self> {
        Ok(Self { pool })
    }

    pub async fn start(&self, mut tls_config_reciever: Receiver<RustlsConfig>) -> Result<()> {
        let config = tls_config_reciever.recv().await?;

        let http_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), HTTP_PORT);
        let https_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), HTTPS_PORT);

        let http = tokio::spawn(Self::http_server(http_addr));
        let https = tokio::spawn(Self::https_server(https_addr, config));

        // Ignore errors.
        let _ = tokio::join!(http, https);

        Ok(())
    }

    async fn http_server(addr: SocketAddr) {
        tracing::info!("HTTP Server listening on {}", addr);

        let redirect = move |Host(host): Host, uri: Uri| async move {
            match make_https(&host, uri, HTTPS_PORT) {
                Ok(uri) => Ok(Redirect::permanent(&uri.to_string())),
                Err(error) => {
                    tracing::warn!(%error, "failed to convert URI to HTTPS");
                    Err(StatusCode::BAD_REQUEST)
                }
            }
        };

        axum_server::bind(addr)
            .serve(redirect.into_make_service())
            .await
            .unwrap();
    }

    async fn https_server(addr: SocketAddr, config: RustlsConfig) {
        tracing::info!("HTTPS Server listening on {}", addr);

        let app_serv = create_router().into_make_service();
        axum_server::bind_rustls(addr, config)
            .serve(app_serv)
            .await
            .unwrap();
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
