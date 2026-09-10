//! Core types and interfaces for the troute v0 routing pipeline.
//!
//! The binary serves the HTTP health endpoint. Concrete routing-provider and
//! optimization implementations are intentionally outside this library skeleton.

pub mod api;
pub mod domain;
pub mod matrix;
pub mod routing;
pub mod schedule;
pub mod service;
pub mod solver;

pub use api::{OptimizeRouteRequest, OptimizeRouteResponse};
pub use service::RouteOptimizationService;
