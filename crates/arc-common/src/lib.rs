//! arc-common
//!
//! 这个 crate 是什么：
//! - 提供整个 workspace 统一的错误类型 `ArcError` 与 `Result<T>`。
//!
//! 这个 crate 不是什么：
//! - 不包含任何网络、协议、路由、插件实现。
//! - 不承担日志/metrics 责任（这些在 arc-observability）。
//!
//! 边界：
//! - 其他 crate 只允许通过 `ArcError` 传播错误，不在库内部传播 `Box<dyn Error>`。

use core::fmt;

/// Workspace-wide result type.
pub type Result<T> = core::result::Result<T, ArcError>;

/// Workspace-wide error type.
///
/// 设计取舍：
/// - 热路径不构造复杂 `String`（大部分运行时错误携带 `&'static str` + `io::Error`）。
/// - 配置加载/编译属于非热路径，可携带 `String` 以提供足够上下文。
#[derive(Debug)]
pub enum ArcError {
    Io {
        op: &'static str,
        err: std::io::Error,
    },
    Config {
        msg: String,
    },
    Proto {
        msg: &'static str,
    },
    Plugin {
        msg: &'static str,
    },
    RateLimit {
        msg: &'static str,
    },
    Observability {
        msg: &'static str,
    },
    Internal {
        msg: String,
    },
}

impl ArcError {
    #[inline]
    pub fn io(op: &'static str, err: std::io::Error) -> Self {
        Self::Io { op, err }
    }

    #[inline]
    pub fn config(msg: String) -> Self {
        Self::Config { msg }
    }

    #[inline]
    pub fn proto(msg: &'static str) -> Self {
        Self::Proto { msg }
    }

    #[inline]
    pub fn plugin(msg: &'static str) -> Self {
        Self::Plugin { msg }
    }

    #[inline]
    pub fn rate_limit(msg: &'static str) -> Self {
        Self::RateLimit { msg }
    }

    #[inline]
    pub fn observability(msg: &'static str) -> Self {
        Self::Observability { msg }
    }

    #[inline]
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal { msg: msg.into() }
    }
}

impl fmt::Display for ArcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArcError::Io { op, err } => write!(f, "io error in {op}: {err}"),
            ArcError::Config { msg } => write!(f, "config error: {msg}"),
            ArcError::Proto { msg } => write!(f, "proto error: {msg}"),
            ArcError::Plugin { msg } => write!(f, "plugin error: {msg}"),
            ArcError::RateLimit { msg } => write!(f, "rate limit error: {msg}"),
            ArcError::Observability { msg } => write!(f, "observability error: {msg}"),
            ArcError::Internal { msg } => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for ArcError {}
