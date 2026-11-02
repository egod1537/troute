mod bootstrap;
mod routing;
mod shutdown;
mod startup;
mod storage;

use std::{error::Error, net::SocketAddr};

use axum::Router;
use tokio::net::TcpListener;
use troute::logging::logger;

use crate::config::AppConfig;

use self::{bootstrap::ServerComponents, shutdown::shutdown_signal};

/// A fully initialized HTTP server that owns its listener and application router.
pub(crate) struct Server {
    bind_addr: SocketAddr,
    listener: TcpListener,
    router: Router,
}

impl Server {
    pub(crate) async fn bind(config: AppConfig) -> Result<Self, Box<dyn Error>> {
        let components = ServerComponents::initialize(config)?;
        let bind_addr = components.bind_addr();
        let listener = TcpListener::bind(bind_addr).await?;
        let router = components.into_router();

        Ok(Self {
            bind_addr,
            listener,
            router,
        })
    }

    pub(crate) async fn serve(self) -> Result<(), Box<dyn Error>> {
        let Self {
            bind_addr,
            listener,
            router,
        } = self;

        logger().server_listening(bind_addr);
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        logger().server_stopped();
        Ok(())
    }
}
