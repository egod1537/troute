mod config;
mod server;

use std::{error::Error, process::ExitCode};

use config::AppConfig;
use server::Server;
use troute::logging::{init as init_logging, logger};

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            logger().server_error(error.as_ref());
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let config = AppConfig::from_env()?;
    let server = Server::bind(config).await?;
    server.serve().await
}
