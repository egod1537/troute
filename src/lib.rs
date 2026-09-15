//! Core types and interfaces for the troute v0 routing pipeline.
//!
//! The binary exposes the v0 pipeline over HTTP. The currently wired routing
//! provider and solver are deterministic development implementations, kept
//! separate so production implementations can replace them later.

pub mod api;
pub mod development;
pub mod domain;
pub mod http;
pub mod matrix;
pub mod routing;
pub mod schedule;
pub mod service;
pub mod solver;

pub use api::{OptimizeRouteRequest, OptimizeRouteResponse};
pub use service::RouteOptimizationService;
