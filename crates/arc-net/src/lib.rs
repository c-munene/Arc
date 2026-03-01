//! arc-net
//!
//! 这个 crate 是什么：
//! - io_uring 封装（mmap ring，SQ/CQ 操作，register buffers/files）
//! - Linux socket 创建与常用 socket 选项（SO_REUSEPORT / TCP_FASTOPEN / TCP_NODELAY 等）
//! - 固定 buffer pool（registered buffers）
//! - 连接 slab（无锁 free-list，generation 防 ABA）
//! - SPSC 队列（消息传递工具）
//! - 单调时钟（clock_gettime）
//!
//! 这个 crate 不是什么：
//! - 不包含 HTTP/路由/插件/限流/配置。
//!
//! 边界：
//! - 上层数据面只通过 arc-net 提供的原语提交 io_uring 操作与管理资源。

pub mod cpu;
pub mod memory;
pub mod net;
pub mod op;
pub mod spsc;
pub mod time;
pub mod uring;

pub use memory::buffers::{FixedBuffers, INVALID_BUF};
pub use memory::slab::{Key, Slab};
pub use uring::ring::Uring;
