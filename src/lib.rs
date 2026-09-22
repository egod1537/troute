//! Core types and interfaces for the troute v0 routing pipeline.
//!
//! The binary exposes the v0 pipeline over HTTP. Routing providers build a
//! directed matrix before exact or heuristic optimization runs. Initial-route
//! generators, including MST Double-Tree and Christofides, consume that matrix
//! without doing travel-time acquisition.

pub mod api;
pub mod cancellation;
pub mod development;
pub mod domain;
pub mod events;
pub mod http;
mod job_pacing;
pub mod jobs;
pub mod matrix;
pub mod observation;
mod result_debug;
pub mod routing;
pub mod schedule;
pub mod service;
pub mod solver;
pub mod storage;
pub mod tcache;

pub use api::{
    ClusterDiagnosticOutput, DebugOptions, MatchingPairDiagnosticOutput, MstEdgeDiagnosticOutput,
    ObjectiveScoreOutput, OptimizeRouteRequest, OptimizeRouteResponse,
    SolverCandidateMetadataOutput, SolverCandidateOutput,
};
pub use cancellation::CancellationToken;
pub use events::{OptimizationErrorCode, OptimizationEventReporter, ProgressStage};
pub use jobs::{JobExecutor, JobRunner};
pub use observation::{
    InMemoryJobTimelineStore, JobObservationRecorder, JobTimelineEntry, JobTimelineStore,
    ObservationDirection, ObservationPeer,
};
pub use service::RouteOptimizationService;
pub use storage::{
    FileJobStore, FileJobTimelineStore, JobIndexEntry, JobState, JobStatus, JobStore,
    JobStoreError, StoredJob, StoredJobError, TerminalWriteOutcome,
};
