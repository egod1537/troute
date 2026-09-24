use std::{env, num::NonZeroU64, time::Duration};

use reqwest::Url;

const DEFAULT_POLL_INTERVAL_MS: u64 = 250;
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
#[derive(Debug, Clone)]
pub struct TcacheRoutingConfig {
    pub base_url: Url,
    pub poll_interval: Duration,
    pub timeout: Duration,
}

impl TcacheRoutingConfig {
    pub fn from_env() -> Result<Self, String> {
        let base_url =
            env::var("TCACHE_BASE_URL").map_err(|_| "TCACHE_BASE_URL is required".to_owned())?;
        if base_url.trim().is_empty() {
            return Err("TCACHE_BASE_URL must not be empty".to_owned());
        }
        let mut base_url = Url::parse(&base_url)
            .map_err(|error| format!("TCACHE_BASE_URL must be a valid URL: {error}"))?;
        if base_url.scheme() != "http" && base_url.scheme() != "https" {
            return Err("TCACHE_BASE_URL must use http or https".to_owned());
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        let poll_interval = read_positive_milliseconds(
            "TCACHE_POLL_INTERVAL_MS",
            "TCACHE_MATRIX_POLL_INTERVAL_MS",
            DEFAULT_POLL_INTERVAL_MS,
        )?;
        let timeout = read_positive_milliseconds(
            "TCACHE_REQUEST_TIMEOUT_MS",
            "TCACHE_MATRIX_TIMEOUT_MS",
            DEFAULT_TIMEOUT_MS,
        )?;
        Ok(Self {
            base_url,
            poll_interval,
            timeout,
        })
    }
}

fn read_positive_milliseconds(
    name: &str,
    legacy_name: &str,
    fallback: u64,
) -> Result<Duration, String> {
    let (name, value) = match env::var(name) {
        Ok(value) => (name, value),
        Err(env::VarError::NotPresent) => match env::var(legacy_name) {
            Ok(value) => (legacy_name, value),
            Err(env::VarError::NotPresent) => return Ok(Duration::from_millis(fallback)),
            Err(error) => return Err(error.to_string()),
        },
        Err(error) => return Err(error.to_string()),
    };
    let value = value
        .parse::<NonZeroU64>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    Ok(Duration::from_millis(value.get()))
}
