use std::{env, num::NonZeroU64, time::Duration};

use reqwest::Url;

pub const DEFAULT_POLL_INTERVAL_MS: u64 = 250;
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
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
        let poll_interval = read_optional("TCACHE_POLL_INTERVAL_MS")?;
        let legacy_poll_interval = read_optional("TCACHE_MATRIX_POLL_INTERVAL_MS")?;
        let timeout = read_optional("TCACHE_REQUEST_TIMEOUT_MS")?;
        let legacy_timeout = read_optional("TCACHE_MATRIX_TIMEOUT_MS")?;
        Self::from_config_sources(
            &base_url,
            poll_interval.as_deref(),
            legacy_poll_interval.as_deref(),
            timeout.as_deref(),
            legacy_timeout.as_deref(),
        )
    }

    /// Resolves aliases and validates raw environment values without reading
    /// process-global state.
    pub fn from_config_sources(
        base_url: &str,
        poll_interval: Option<&str>,
        legacy_poll_interval: Option<&str>,
        timeout: Option<&str>,
        legacy_timeout: Option<&str>,
    ) -> Result<Self, String> {
        let poll_interval = parse_positive_milliseconds(
            "TCACHE_POLL_INTERVAL_MS",
            poll_interval,
            "TCACHE_MATRIX_POLL_INTERVAL_MS",
            legacy_poll_interval,
            DEFAULT_POLL_INTERVAL_MS,
        )?;
        let timeout = parse_positive_milliseconds(
            "TCACHE_REQUEST_TIMEOUT_MS",
            timeout,
            "TCACHE_MATRIX_TIMEOUT_MS",
            legacy_timeout,
            DEFAULT_TIMEOUT_MS,
        )?;
        Self::from_parts(base_url, poll_interval, timeout)
    }

    /// Builds a validated tcache configuration from already resolved values.
    /// This lets application configuration load the process environment once
    /// while preserving `from_env` as a compatibility entry point.
    pub fn from_parts(
        base_url: &str,
        poll_interval: Duration,
        timeout: Duration,
    ) -> Result<Self, String> {
        if base_url.trim().is_empty() {
            return Err("TCACHE_BASE_URL must not be empty".to_owned());
        }
        let mut base_url = Url::parse(base_url)
            .map_err(|error| format!("TCACHE_BASE_URL must be a valid URL: {error}"))?;
        if base_url.scheme() != "http" && base_url.scheme() != "https" {
            return Err("TCACHE_BASE_URL must use http or https".to_owned());
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        Ok(Self {
            base_url,
            poll_interval,
            timeout,
        })
    }
}

fn read_optional(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn parse_positive_milliseconds(
    name: &str,
    value: Option<&str>,
    legacy_name: &str,
    legacy_value: Option<&str>,
    fallback: u64,
) -> Result<Duration, String> {
    let (name, value) = match (value, legacy_value) {
        (Some(value), _) => (name, value),
        (None, Some(value)) => (legacy_name, value),
        (None, None) => return Ok(Duration::from_millis(fallback)),
    };
    let value = value
        .parse::<NonZeroU64>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    Ok(Duration::from_millis(value.get()))
}
