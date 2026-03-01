# Benchmarks

本文件只提供**方法与口径**，不伪造任何结果。请在目标机器跑出真实数据并填入 Result 表。

---

## 1. 环境要求（建议）

- Linux kernel: >= 6.1（更高版本通常对 io_uring / multishot 更友好）
- CPU：至少 8C/16T
- NIC：10GbE（更好）
- backend：同机 loopback 或独立机器（建议两机）
- 工具：
  - `wrk` 或 `wrk2`
  - `nginx`（对比）
  - `taskset` / `perf` / `bpftool`（可选）

---

## 2. 场景定义（验收标准口径）

### A) fast backend（吞吐）
- backend：返回固定小响应（例如 64B body），keep-alive on
- 并发：例如 256/512/1024
- 指标：
  - RPS、p50/p99 latency
  - CPU 使用率（每核）
  - error rate（必须为 0）

### B) slow backend（zero error）
- backend：每请求 sleep 50ms/100ms（可控）
- 并发：例如 10k keep-alive 连接
- 指标：
  - error rate（硬要求：0）
  - 连接生命周期计数是否稳定
  - phase timeout 统计是否符合预期

---

## 3. 对比对象

- Nginx（同机配置：worker_processes auto; keepalive; proxy_* 参数一致）
- 旧 Arc（Pingora 版）——如果你仍保留旧实现可跑的话

---

## 4. 推荐命令模板

### wrk（fast backend）
```bash
wrk -t8 -c512 -d30s --latency http://127.0.0.1:8080/
```

### wrk2（恒定速率）
```bash
wrk -t8 -c512 -d30s -R200000 --latency http://127.0.0.1:8080/
```

---

## 5. 结果表（请填真实数据）

| Scenario | Arc Gateway RPS | Nginx RPS | Old Arc(Pingora) RPS | Error Rate |
|---------|------------------|-----------|-----------------------|------------|
| fast backend | TBD | TBD | TBD | 0 |
| slow backend | TBD | TBD | TBD | 0 |

---

## 6. 性能诊断建议（可选）

- `perf stat -a -- sleep 10`
- `perf top`
- `/metrics` 中关注：
  - phase_time_sum_ns / phase_count
  - phase_timeout_total
  - ring_sq_dropped / ring_cq_overflow
