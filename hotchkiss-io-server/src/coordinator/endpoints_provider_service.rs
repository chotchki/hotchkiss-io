use crate::web::router::create_router;
use anyhow::Result;
use axum_server::tls_rustls::RustlsConfig;
use sqlx::SqlitePool;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::sync::broadcast::Receiver;

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

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), HTTPS_PORT);

        let app_serv = create_router().into_make_service();
        let builder = axum_server::bind_rustls(addr, config);

        tracing::info!("HTTPS Server listening on {}", addr);
        builder.serve(app_serv).await?;

        Ok(())
    }
}
