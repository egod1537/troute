mod config;
mod server;

use std::{error::Error, process::ExitCode};

use config::AppConfig;
use server::Server;
use troute::logging::{init as init_logging, logger};

fn main() -> ExitCode {
    init_logging();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            logger().server_error(error.as_ref());
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut config = AppConfig::from_env()?;
    let cpu = troute::affinity::reserve_validation_cpu(config.server.validation_cpu_policy)?;
    config.server.router.validation_cpu = cpu;
    if let Some(cpu) = cpu {
        eprintln!("Fast validation reserved logical CPU: {cpu}; ordinary runtime and solver threads exclude it");
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let server = Server::bind(config).await?;
            server.serve().await
        })
}
