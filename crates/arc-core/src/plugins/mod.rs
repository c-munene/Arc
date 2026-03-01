//! Extension engines.
//!
//! Arc supports:
//! - WASM: sandboxed, multi-language, hot-loadable
//! - Rhai: ultra-lightweight inline scripting

pub mod rhai;
#[cfg(feature = "wasm")]
pub mod wasm;
