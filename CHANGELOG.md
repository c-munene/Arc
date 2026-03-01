# Changelog

## 2026-02-21 — io_uring data plane unified

### Added
- `arc-gateway`：唯一数据面二进制，thread-per-core + io_uring。
- 路由（Radix Tree）、限流（原子 CAS token-bucket 等价）、WASM 插件（Wasmtime + 实例池 + epoch 超时）。
- 内置可观测性：phase 级别耗时/超时统计、连接生命周期计数、Prometheus `/metrics`。

### Changed
- 配置系统重写为 JSON（最低优先级允许重设计）。后台线程编译完成后使用 `ArcSwap` 原子替换。

### Removed
- Pingora 相关依赖与 data plane：`crates/arc-daemon`（原 Pingora embedding）、`vendor/pingora-*`（迁移后应删除）。
- 旧的 Pingora glue、旧的 metrics endpoint 逻辑。

### Notes
- 本版本聚焦 HTTP/1.1 明文与核心功能闭环：吞吐、zero-error、插件链、限流、热重载、观测。
- TLS/HTTP2 不在本次范围内（可作为后续扩展）。
