//! Memory utilities used by dataplane.
//!
//! 边界：
//! - 提供固定 buffer pool 与 slab。
//! - 不包含协议语义。

pub mod buffers;
pub mod slab;
