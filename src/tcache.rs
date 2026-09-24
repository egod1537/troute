//! Backward-compatible exports for the tcache routing adapter.
//!
//! New code may use `crate::providers::tcache`; existing callers can continue
//! using `crate::tcache`.

pub use crate::providers::tcache::{TcacheRoutingConfig, TcacheTravelTimeProvider};
