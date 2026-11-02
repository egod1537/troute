use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::cancellation::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub logical_cpu_slots: usize,
    pub max_concurrent_runs: usize,
    pub max_external_processes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub logical_cpu_slots: usize,
    pub native_threads: usize,
    pub external_processes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_hint_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub logical_cpu_slots: usize,
    pub active_runs: usize,
    pub external_processes: usize,
    pub memory_hint_bytes: u64,
}

#[derive(Debug)]
struct ResourceState {
    usage: ResourceUsage,
}

#[derive(Debug)]
struct ResourceInner {
    limits: ResourceLimits,
    state: Mutex<ResourceState>,
    released: Condvar,
}

#[derive(Debug, Clone)]
pub struct ResourceManager {
    inner: Arc<ResourceInner>,
}

impl ResourceManager {
    pub fn new(limits: ResourceLimits) -> Result<Self, ResourceError> {
        if limits.logical_cpu_slots == 0 || limits.max_concurrent_runs == 0 {
            return Err(ResourceError::InvalidLimits);
        }
        Ok(Self {
            inner: Arc::new(ResourceInner {
                limits,
                state: Mutex::new(ResourceState {
                    usage: ResourceUsage::default(),
                }),
                released: Condvar::new(),
            }),
        })
    }

    pub fn limits(&self) -> ResourceLimits {
        self.inner.limits
    }

    pub fn usage(&self) -> ResourceUsage {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .usage
    }

    pub fn try_acquire(&self, request: ResourceRequest) -> Result<ResourceLease, ResourceError> {
        self.validate_request(request)?;
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !fits(self.inner.limits, state.usage, request) {
            return Err(ResourceError::Unavailable);
        }
        reserve(&mut state.usage, request);
        Ok(ResourceLease {
            inner: Some(self.inner.clone()),
            request,
            acquired_at: Instant::now(),
        })
    }

    pub fn acquire_until(
        &self,
        request: ResourceRequest,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ResourceLease, ResourceError> {
        self.validate_request(request)?;
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if cancellation.is_cancelled() {
                return Err(ResourceError::Cancelled);
            }
            if fits(self.inner.limits, state.usage, request) {
                reserve(&mut state.usage, request);
                return Ok(ResourceLease {
                    inner: Some(self.inner.clone()),
                    request,
                    acquired_at: Instant::now(),
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ResourceError::DeadlineExceeded);
            }
            let wait = remaining.min(Duration::from_millis(5));
            let (next, _) = self
                .inner
                .released
                .wait_timeout(state, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
    }

    fn validate_request(&self, request: ResourceRequest) -> Result<(), ResourceError> {
        if request.logical_cpu_slots == 0
            || request.native_threads == 0
            || request.native_threads > request.logical_cpu_slots
            || request.logical_cpu_slots > self.inner.limits.logical_cpu_slots
            || request.external_processes > self.inner.limits.max_external_processes
            || matches!(
                (request.memory_hint_bytes, self.inner.limits.memory_limit_bytes),
                (Some(requested), Some(limit)) if requested > limit
            )
        {
            return Err(ResourceError::InvalidRequest);
        }
        Ok(())
    }
}

fn fits(limits: ResourceLimits, usage: ResourceUsage, request: ResourceRequest) -> bool {
    usage.active_runs < limits.max_concurrent_runs
        && usage
            .logical_cpu_slots
            .saturating_add(request.logical_cpu_slots)
            <= limits.logical_cpu_slots
        && usage
            .external_processes
            .saturating_add(request.external_processes)
            <= limits.max_external_processes
        && match limits.memory_limit_bytes {
            Some(limit) => {
                usage
                    .memory_hint_bytes
                    .saturating_add(request.memory_hint_bytes.unwrap_or(0))
                    <= limit
            }
            None => true,
        }
}

fn reserve(usage: &mut ResourceUsage, request: ResourceRequest) {
    usage.active_runs += 1;
    usage.logical_cpu_slots += request.logical_cpu_slots;
    usage.external_processes += request.external_processes;
    usage.memory_hint_bytes = usage
        .memory_hint_bytes
        .saturating_add(request.memory_hint_bytes.unwrap_or(0));
}

#[derive(Debug)]
pub struct ResourceLease {
    inner: Option<Arc<ResourceInner>>,
    request: ResourceRequest,
    acquired_at: Instant,
}

impl ResourceLease {
    pub fn request(&self) -> ResourceRequest {
        self.request
    }

    pub fn reserved_slot_ms(&self) -> u64 {
        let millis = self.acquired_at.elapsed().as_millis();
        u64::try_from(millis)
            .unwrap_or(u64::MAX)
            .saturating_mul(self.request.logical_cpu_slots as u64)
    }
}

impl Drop for ResourceLease {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        let mut state = inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.usage.active_runs = state.usage.active_runs.saturating_sub(1);
        state.usage.logical_cpu_slots = state
            .usage
            .logical_cpu_slots
            .saturating_sub(self.request.logical_cpu_slots);
        state.usage.external_processes = state
            .usage
            .external_processes
            .saturating_sub(self.request.external_processes);
        state.usage.memory_hint_bytes = state
            .usage
            .memory_hint_bytes
            .saturating_sub(self.request.memory_hint_bytes.unwrap_or(0));
        inner.released.notify_all();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ResourceError {
    #[error("resource limits must have positive CPU slots and run capacity")]
    InvalidLimits,
    #[error("resource request exceeds a configured limit")]
    InvalidRequest,
    #[error("resources are currently unavailable")]
    Unavailable,
    #[error("resource wait was cancelled")]
    Cancelled,
    #[error("resource wait exceeded its deadline")]
    DeadlineExceeded,
}
