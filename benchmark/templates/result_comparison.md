# Benchmark 对比模板

## 基本信息

- 时间：
- Arc commit：
- 测试机器：
- 内核版本：
- 测试场景：H1(wrk) / H2(h2load)

## 参数

- 并发参数：
- 持续时间 / 请求总量：
- 轮次：
- 后端实现：

## 结果（中位数）

| case | rps | latency / success | 备注 |
|---|---:|---:|---|
| arc |  |  |  |
| nginx |  |  |  |
| backend（如有） |  |  |  |

## 对比结论

- arc_vs_nginx_rps_ratio_median：
- arc_vs_nginx_rps_gap_pct_median：

## 复现材料

- `env.txt`
- `summary.json`
- `summary.md`
- 每轮原始输出（`*_run1.txt` ...）

