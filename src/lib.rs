//! Core types and interfaces for the troute v0 routing pipeline.
//!
//! Concrete HTTP, routing-provider, and optimization implementations are
//! intentionally outside the initial project skeleton.

pub mod api;
pub mod domain;
pub mod matrix;
pub mod routing;
pub mod schedule;
pub mod service;
pub mod solver;

pub use api::{OptimizeRouteRequest, OptimizeRouteResponse};
pub use service::RouteOptimizationService;
