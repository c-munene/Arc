//! Arc logging subsystem (standalone crate).
//!
//! Design goals (per Arc spec):
//! - Worker hot path: thread-local lock-free ring buffer push (< 50ns target).
//! - Ring full => drop + counter; never backpressure workers.
//! - Tail-based sampling decided at request end.
//! - NDJSON only.
//! - File rotation + optional gzip compression.
//! - System logs bypass ring, always 100% recorded via async channel.
//! - Debug logs compile-time gated: when feature is off, code physically absent.
//! - W3C trace context helper (`traceparent`) for parse/generate/propagate.
//!
//! Context model:
//! - ring target is thread-local (per worker).
//! - request context should be task-local or explicit request struct in async runtimes.
//!
//! Integration model:
//! - `init_global(workers, runtime_cfg)` once in process.
//! - Each worker thread calls `init_worker(wid)` once.
//! - Request path holds an `AccessLogContext` (~200-500B typical) and calls `submit_access_*` at request end.
//! - Control plane can call `set_route_override` and `status_json`.
//! - Metrics endpoint can call `global_metrics_render_prometheus()` and append to existing output.

#![allow(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod config;
mod escape;
mod metrics;
mod record;
mod redact;
mod ring;
mod runtime;
mod trace;
mod util;
mod writer;

pub use crate::config::{
    AccessConfig, LoggingRuntimeConfig, OutputConfig, RedactConfig, RotationConfig, WriterConfig,
};
pub use crate::metrics::{LogMetrics, LOG_WRITE_DURATION_BUCKETS_SECONDS};
pub use crate::record::{
    AccessErrorLogRecord, AccessLogContext, AccessLogRecord, LogEvent, LogLevel, LogStr, LogValue,
    RequestContextView, SystemLogRecord,
};
pub use crate::runtime::{
    enter_request_scope, global, global_metrics_render_prometheus, init_global,
    init_global_from_raw_json, init_worker, is_initialized, set_route_override, status_json,
    submit_access_error, submit_access_success, system_log, system_log_fields, system_log_kv,
    LoggingError, LoggingHandle, Result,
};
pub use crate::trace::TraceContext;

#[cfg(feature = "debug_log")]
pub use crate::runtime::{debug_log, debug_log_fields};

/// Debug log macro (feature-gated).
///
/// When `debug_log` feature is **disabled**, this expands to nothing (zero cost).
#[macro_export]
macro_rules! arc_debug_log {
    ($level:expr, $msg:expr) => {{
        #[cfg(feature = "debug_log")]
        {
            $crate::debug_log($level, $msg);
        }
        #[cfg(not(feature = "debug_log"))]
        {
            let _ = &$level;
            let _ = &$msg;
        }
    }};
}

/// Debug log macro with fields (feature-gated).
///
/// When `debug_log` feature is **disabled**, this expands to nothing (zero cost).
#[macro_export]
macro_rules! arc_debug_log_fields {
    ($level:expr, $msg:expr, $fields:expr) => {{
        #[cfg(feature = "debug_log")]
        {
            $crate::debug_log_fields($level, $msg, $fields);
        }
        #[cfg(not(feature = "debug_log"))]
        {
            let _ = &$level;
            let _ = &$msg;
            let _ = &$fields;
        }
    }};
}
