//! arc-xdp-userspace
//!
//! 这个 crate 是什么：
//! - Arc 网关的 XDP 用户态集成层（不修改任何 eBPF 程序）。
//! - 直接消费 pinned map + ringbuf 事件（默认路径 `/sys/fs/bpf/arc/`，可由 `xdp.pin_base` 覆盖）。
//! - 提供：XdpManager / 黑白名单管理 / ringbuf 消费 / 动态阈值计算 / stats 聚合 / L7 防护 / zerocopy / system tune。
//!
//! 不是什么：
//! - 不负责实现或改动 eBPF 程序本体。
//! - 不负责 Arc 控制面路由（control.rs）和 Arc CLI（若存在独立 crate，需要在调用方接入）。
//!
//! 设计原则：
//! - XDP Disabled 模式必须可用且不崩溃：所有 API 都返回明确状态。
//! - 热路径（worker）不依赖这里的锁；这里的任务都在后台 tokio runtime 执行。

#![cfg(target_os = "linux")]
#![allow(dead_code, unused_variables, unused_mut, unused_assignments)]

pub mod bpf;
pub mod config;
pub mod l7;
pub mod manager;
pub mod system_tuner;
pub mod zerocopy;

pub use crate::config::{ArcSecurityConfig, L7ProtectionConfig, XdpUserConfig};
pub use crate::l7::{H2StreamFloodGuard, L7Protection, SlowlorisGuard, TlsFloodGuard};
pub use crate::manager::{
    BlacklistManager, L7LinkHandle, StatsSnapshot, ThresholdSnapshot, WhitelistManager, XdpManager,
    XdpManagerState, XdpMode, XdpStatusSnapshot,
};
pub use crate::system_tuner::{SystemStatusSnapshot, SystemTuner, TuneMode, TuneResult};
pub use crate::zerocopy::ZeroCopyResponder;

/// XDP pinned objects base directory default.
pub const XDP_PIN_BASE: &str = "/sys/fs/bpf/arc";
