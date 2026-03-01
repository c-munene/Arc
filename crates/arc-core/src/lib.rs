//! Arc Core
//!
//! This crate contains **all** data-plane and control-plane logic that is *not* tied
//! to a specific network engine. The Arc daemon embeds Pingora for L7 (HTTP/1+2 + TLS)
//! and reuses Arc Core for routing, upstream management, governance, plugins,
//! observability, and hot-reload.
//!
//! Design goals:
//! - No global locks on the request path
//! - RCU-style hot reload via `arc-swap`
//! - Allocation discipline: minimize per-request and per-connection allocations
//! - Cross-platform: Linux + Windows

// Arc is a large system; we keep docs strict at the public API boundary,
// and allow internal modules to evolve without blocking builds.
#![allow(missing_docs)]

pub mod config;
pub mod compiled;
pub mod router;
pub mod upstream;
pub mod rate_limit;
pub mod mutations;
pub mod trace;
pub mod telemetry;
pub mod control;
pub mod plugins;
pub mod policy_mirror;
pub mod policy_timeout;

pub use compiled::{CompiledConfig, SharedConfig};
