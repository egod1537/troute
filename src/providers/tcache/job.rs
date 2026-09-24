use std::{thread, time::Instant};

use chrono::{SecondsFormat, Utc};

use crate::{
    cancellation::CancellationToken,
    domain::Location,
    routing::{RoutingContext, RoutingError, TravelTime, TravelTimeProvider},
};

use super::{
    client::{cancelled_error, pair_context, timeout_error, TcacheTravelTimeProvider},
    travel_time::{
        extract_travel_time, CreateJobResponse, JobStatusResponse, RouteLocation, RouteRequest,
        RouteResultEnvelope,
    },
};

impl TcacheTravelTimeProvider {
    fn travel_time_inner(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
        cancellation: Option<&CancellationToken>,
        matrix_deadline: Option<Instant>,
    ) -> Result<TravelTime, RoutingError> {
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(cancelled_error(from, to, None));
        }
        let pair_deadline = Instant::now() + self.config.timeout;
        let deadline = matrix_deadline
            .map(|matrix_deadline| matrix_deadline.min(pair_deadline))
            .unwrap_or(pair_deadline);
        let endpoint = self.endpoint("api/route/jobs")?;
        let request = RouteRequest {
            locations: vec![RouteLocation::from(from), RouteLocation::from(to)],
            mode: context.travel_mode.as_provider_value(),
            departure_time: context
                .departure_time
                .unwrap_or_else(Utc::now)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            language_code: context.options.get("languageCode").map(String::as_str),
            region_code: context.options.get("regionCode").map(String::as_str),
            routing_preference: context.options.get("routingPreference").map(String::as_str),
            units: context.options.get("units").map(String::as_str),
        };
        let created: CreateJobResponse = self.request_json(
            self.client.post(endpoint.clone()).json(&request),
            &endpoint,
            deadline,
            from,
            to,
            None,
        )?;
        if created.job_id.trim().is_empty() {
            return Err(RoutingError::Provider(format!(
                "tcache returned an empty jobId from {endpoint}{}",
                pair_context(from, to)
            )));
        }
        let job_id = created.job_id;
        let status_endpoint = self.endpoint(&format!("api/route/jobs/{job_id}"))?;

        loop {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                self.cancel(&job_id);
                return Err(cancelled_error(from, to, Some(&job_id)));
            }
            if Instant::now() >= deadline {
                self.cancel(&job_id);
                eprintln!(
                    "tcache route job timed out; pair={} -> {}; jobId={job_id}",
                    from.id(),
                    to.id()
                );
                return Err(timeout_error(from, to, Some(&job_id)));
            }
            let status: JobStatusResponse = match self.request_json(
                self.client.get(status_endpoint.clone()),
                &status_endpoint,
                deadline,
                from,
                to,
                Some(&job_id),
            ) {
                Ok(status) => status,
                Err(_error) if Instant::now() >= deadline => {
                    self.cancel(&job_id);
                    eprintln!(
                        "tcache route job timed out; pair={} -> {}; jobId={job_id}",
                        from.id(),
                        to.id()
                    );
                    return Err(timeout_error(from, to, Some(&job_id)));
                }
                Err(error) => return Err(error),
            };
            match status.status.as_str() {
                "completed" => break,
                "failed" | "cancelled" => {
                    let detail = status
                        .error
                        .map(|error| format!("{}: {}", error.code, error.message))
                        .unwrap_or_else(|| status.status.clone());
                    return Err(RoutingError::Provider(format!(
                        "tcache route job {job_id} {}{pair}: {detail}",
                        status.status,
                        pair = pair_context(from, to),
                    )));
                }
                "queued" | "running" => {}
                other => {
                    return Err(RoutingError::Provider(format!(
                        "tcache route job {job_id} returned unknown status {other}{}",
                        pair_context(from, to)
                    )))
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                continue;
            }
            thread::sleep(self.config.poll_interval.min(remaining));
        }

        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            self.cancel(&job_id);
            return Err(cancelled_error(from, to, Some(&job_id)));
        }
        let result_endpoint = self.endpoint(&format!("api/route/jobs/{job_id}/result"))?;
        let result: RouteResultEnvelope = match self.request_json(
            self.client.get(result_endpoint.clone()),
            &result_endpoint,
            deadline,
            from,
            to,
            Some(&job_id),
        ) {
            Ok(result) => result,
            Err(_error) if Instant::now() >= deadline => {
                self.cancel(&job_id);
                eprintln!(
                    "tcache route job timed out; pair={} -> {}; jobId={job_id}",
                    from.id(),
                    to.id()
                );
                return Err(timeout_error(from, to, Some(&job_id)));
            }
            Err(error) => return Err(error),
        };
        extract_travel_time(&job_id, from, to, result)
    }
}

impl TravelTimeProvider for TcacheTravelTimeProvider {
    fn travel_time(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError> {
        self.travel_time_inner(from, to, context, None, None)
    }

    fn travel_time_with_cancellation(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
        cancellation: &CancellationToken,
    ) -> Result<TravelTime, RoutingError> {
        self.travel_time_inner(from, to, context, Some(cancellation), None)
    }

    fn travel_time_with_cancellation_until(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
        cancellation: &CancellationToken,
        deadline: Instant,
    ) -> Result<TravelTime, RoutingError> {
        self.travel_time_inner(from, to, context, Some(cancellation), Some(deadline))
    }
}
