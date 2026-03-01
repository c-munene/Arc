# Contributing to Arc

感谢您愿意参与 Arc。  
这份文档只覆盖最重要、最常用的贡献流程。

## 环境要求

- Rust：稳定版（建议 `1.75+`，并保持与 CI 一致）
- 操作系统：Linux（数据面主路径依赖 `io_uring`）
- 依赖工具：
  - `cargo`
  - `wrk`（HTTP/1.1 压测）
  - `h2load`（HTTP/2 压测）
  - `hping3`（网络/流量场景验证）
  - `python3`（烟雾脚本和 benchmark 后端）
  - `nginx`（基线对比）

### 注意事项

- Windows 本地开发建议使用 WSL2 的 Linux 环境运行网关和压测。
- 请优先在 WSL2 内执行 `cargo`、`wrk`、`h2load`，不要混用 Windows 与 WSL2 的工具链。

## 构建

```bash
cargo build
cargo check
cargo test
```

只改某个 crate 时，建议先跑定向命令：

```bash
cargo check -p arc-gateway
cargo test -p arc-config
```

## 测试

### 单测

- 全量：`cargo test`
- 定向：`cargo test -p <crate> <test_name>`

### 烟雾测试

- 仓库已有脚本主要放在 `tmp/`，按场景命名。
- 示例：
  - `tmp/run_h1_body_limit_smoke.sh`
  - `tmp/run_phase_rr_smoke.sh`
  - `tmp/run_h2_finish_smoke.sh`

执行前请先确认脚本里的端口和路径在本机可用。

### Benchmark

统一使用 `benchmark/` 目录的脚本，避免参数不一致：

```bash
bash benchmark/scripts/run_h1_wrk_vs_nginx.sh
bash benchmark/scripts/run_h2_h2load_vs_nginx.sh
```

输出会落到 `benchmark/results/<run_id>/`，包括：

- 原始输出
- `env.txt`
- `summary.json`
- `summary.md`

## PR 规范

### Commit message

建议使用 Conventional Commits：

- `feat: ...`
- `fix: ...`
- `refactor: ...`
- `docs: ...`
- `test: ...`
- `chore: ...`

### 必须通过的检查项

- `cargo check` 通过
- 受影响 crate 的单测通过
- 对应场景烟雾测试通过（如协议、路由、限流、日志）
- 改动涉及性能路径时，附带 benchmark 结果目录

### 不接受的改动类型

- 不接受未评审的“自研 TLS 协议栈”或替换现有 `rustls` 安全边界的实现
- 不接受绕过现有配置校验链路的快捷入口
- 不接受没有验证数据的性能结论（只给口头结论、不附原始输出）
- 不接受与当前任务无关的大范围重构

## 架构说明

### 各 crate 职责

- `arc-gateway`：数据面主程序，负责连接管理、协议处理、转发与回写。
- `arc-config`：配置解析、编译、热重载与策略落地。
- `arc-router`：路径匹配与候选路由选择。
- `arc-proto-http1`：HTTP/1.1 报文解析与状态机。
- `arc-proto-h2`：HTTP/2 协议处理基础能力。
- `arc-logging`：访问日志与日志写入管线。
- `arc-observability`：指标与管理面可观测能力。
- `arc-rate-limit`：本地限流基础组件。
- `arc-global-rate-limit`：全局限流（含 Redis 后端）。
- `arc-compression`：压缩策略与编码实现。
- `arc-plugins`：插件加载与执行管理。
- `arc-acme`：证书自动化相关能力。
- `arc-net`：网络辅助能力与底层封装。
- `arc-common`：通用错误类型与基础共享定义。
- `arc-cli`：命令行相关入口与辅助命令。
- `arc-xdp-userspace`：XDP 用户态控制与交互。
- `arc-xdp`：XDP 程序相关组件。
- `arc-xdp-common`：XDP 共享数据结构与协议定义。
- `arc-core`：核心抽象（实验/演进中的公共层）。
- `arc-daemon`：守护进程相关能力（演进中）。

### 热路径位置

- `crates/arc-gateway/src/worker.rs`
- `crates/arc-proto-http1/src/lib.rs`
- `crates/arc-proto-h2/src/`

涉及热路径的改动，请附带性能对比数据，不要只给理论分析。

### 另外

- 不要破坏现有 `io_uring` 线程模型与事件循环假设。
- 不要在热路径引入高频分配或阻塞 IO。
- 不要让控制面逻辑侵入数据面热循环。
- 变更协议解析行为时，必须补单测与回归验证。

