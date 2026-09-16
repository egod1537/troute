//! Core types and interfaces for the troute v0 routing pipeline.
//!
//! The binary exposes the v0 pipeline over HTTP. The currently wired routing
//! provider and solver are deterministic development implementations, kept
//! separate so production implementations can replace them later.

pub mod api;
pub mod cancellation;
pub mod development;
pub mod domain;
pub mod events;
pub mod http;
pub mod matrix;
pub mod observation;
pub mod routing;
pub mod schedule;
pub mod service;
pub mod solver;
pub mod storage;
pub mod trasolve;

pub use api::{OptimizeRouteRequest, OptimizeRouteResponse};
pub use cancellation::CancellationToken;
pub use events::{
    CancelledEventData, ErrorEventData, JobEvent, JobEventContext, JobEventType,
    OptimizationErrorCode, OptimizationEventReporter, ProgressEventData, ProgressStage,
};
pub use observation::{
    InMemoryJobTimelineStore, JobObservationRecorder, JobTimelineEntry, JobTimelineStore,
    ObservationDirection, ObservationPeer,
};
pub use service::RouteOptimizationService;
pub use storage::{
    FileJobStore, FileJobTimelineStore, JobIndexEntry, JobState, JobStatus, JobStore,
    JobStoreError, StoredJob, StoredJobError, TerminalWriteOutcome,
};
