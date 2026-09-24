use std::{
    error::Error,
    time::{Duration, Instant},
};

use reqwest::{
    blocking::{Client, Response},
    Url,
};
use serde::de::DeserializeOwned;

use crate::{domain::Location, routing::RoutingError};

use super::{
    config::TcacheRoutingConfig,
    travel_time::{ErrorEnvelope, TcacheErrorBody},
};

const MAX_ERROR_BODY_CHARACTERS: usize = 2_048;

#[derive(Debug, Clone)]
pub struct TcacheTravelTimeProvider {
    pub(super) client: Client,
    pub(super) config: TcacheRoutingConfig,
}

impl TcacheTravelTimeProvider {
    pub fn new(config: TcacheRoutingConfig) -> Result<Self, RoutingError> {
        let client = Client::builder()
            .connect_timeout(config.timeout)
            .timeout(config.timeout)
            .build()
            .map_err(|error| {
                RoutingError::Provider(format!("cannot build tcache HTTP client: {error}"))
            })?;
        Ok(Self { client, config })
    }

    pub(super) fn endpoint(&self, path: &str) -> Result<Url, RoutingError> {
        self.config.base_url.join(path).map_err(|error| {
            RoutingError::Provider(format!("cannot build tcache endpoint {path}: {error}"))
        })
    }

    pub(super) fn request_json<T: DeserializeOwned>(
        &self,
        request: reqwest::blocking::RequestBuilder,
        endpoint: &Url,
        deadline: Instant,
        from: &Location,
        to: &Location,
        job_id: Option<&str>,
    ) -> Result<T, RoutingError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timeout_error(from, to, job_id));
        }
        let response = match request.timeout(remaining).send() {
            Ok(response) => response,
            Err(_error) if Instant::now() >= deadline => {
                return Err(timeout_error(from, to, job_id));
            }
            Err(error) => return Err(http_error(endpoint, from, to, job_id, error)),
        };
        decode_response(response, endpoint, from, to, job_id)
    }

    pub(super) fn cancel(&self, job_id: &str) {
        let Ok(endpoint) = self.endpoint(&format!("api/route/jobs/{job_id}/cancel")) else {
            return;
        };
        let timeout = self
            .config
            .poll_interval
            .clamp(Duration::from_millis(100), Duration::from_secs(2));
        let _ = self.client.post(endpoint).timeout(timeout).send();
    }
}

fn decode_response<T: DeserializeOwned>(
    response: Response,
    endpoint: &Url,
    from: &Location,
    to: &Location,
    job_id: Option<&str>,
) -> Result<T, RoutingError> {
    let status = response.status();
    let body = response.text().map_err(|error| {
        RoutingError::Provider(format!(
            "cannot read tcache response from {endpoint}{}{}: {error}",
            pair_context(from, to),
            job_context(job_id)
        ))
    })?;
    if !status.is_success() {
        let parsed = serde_json::from_str::<ErrorEnvelope>(&body).ok();
        if let Some(error) = parsed
            .as_ref()
            .and_then(|envelope| provider_configuration_error(&envelope.error, None, None))
        {
            return Err(error);
        }
        let detail = parsed
            .map(|envelope| format!("{}: {}", envelope.error.code, envelope.error.message))
            .unwrap_or_else(|| truncate(&body));
        return Err(RoutingError::Provider(format!(
            "tcache request to {endpoint}{}{} failed with HTTP {}: {detail}",
            pair_context(from, to),
            job_context(job_id),
            status.as_u16()
        )));
    }
    serde_json::from_str(&body).map_err(|error| {
        RoutingError::Provider(format!(
            "cannot decode tcache JSON from {endpoint}{}{}: {error}",
            pair_context(from, to),
            job_context(job_id)
        ))
    })
}

pub(super) fn provider_configuration_error(
    error: &TcacheErrorBody,
    provider: Option<&str>,
    mode: Option<&str>,
) -> Option<RoutingError> {
    if error.code.eq_ignore_ascii_case("PROVIDER_NOT_CONFIGURED") {
        return Some(RoutingError::ProviderNotConfigured {
            provider: provider.unwrap_or("selected").to_owned(),
            reason: error.message.clone(),
        });
    }
    if error
        .code
        .eq_ignore_ascii_case("UNSUPPORTED_PROVIDER_CAPABILITY")
    {
        return Some(RoutingError::UnsupportedProviderCapability {
            provider: provider.unwrap_or("selected").to_owned(),
            mode: mode.unwrap_or("selected").to_owned(),
        });
    }
    None
}

pub(super) fn timeout_error(from: &Location, to: &Location, job_id: Option<&str>) -> RoutingError {
    RoutingError::Provider(format!(
        "tcache route request timed out{}{}",
        pair_context(from, to),
        job_context(job_id),
    ))
}

pub(super) fn cancelled_error(
    from: &Location,
    to: &Location,
    job_id: Option<&str>,
) -> RoutingError {
    RoutingError::Provider(format!(
        "tcache route request cancelled{}{}",
        pair_context(from, to),
        job_context(job_id),
    ))
}

pub(super) fn pair_context(from: &Location, to: &Location) -> String {
    format!(" for pair {} -> {}", from.id(), to.id())
}

fn http_error(
    endpoint: &Url,
    from: &Location,
    to: &Location,
    job_id: Option<&str>,
    error: reqwest::Error,
) -> RoutingError {
    RoutingError::Provider(format!(
        "tcache HTTP request to {endpoint}{}{} failed: {}",
        pair_context(from, to),
        job_context(job_id),
        error_with_causes(&error),
    ))
}

fn error_with_causes(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

fn job_context(job_id: Option<&str>) -> String {
    job_id
        .map(|id| format!(" for job {id}"))
        .unwrap_or_default()
}

fn truncate(value: &str) -> String {
    value.chars().take(MAX_ERROR_BODY_CHARACTERS).collect()
}
