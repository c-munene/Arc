# Arc Gateway

> [!WARNING]
> Documentation is still in progress and test coverage is still expanding.
> Please be patient while updates are being finalized.

A high-performance HTTP reverse proxy built on Linux io_uring, designed for modern cloud-native environments.

Arc delivers 2x the throughput of Nginx in HTTP/1.1 proxy scenarios while maintaining zero-error guarantees under load. It combines a thread-per-core architecture with lock-free algorithms to minimize latency and maximize resource efficiency.

## Performance

Benchmarked on WSL2 Debian (2 vCPU):

| Scenario | Arc | Nginx | Ratio |
|----------|-----|-------|-------|
| HTTP/1.1 proxy (c=256) | 102,203 RPS | 49,930 RPS | **2.05x** |
| HTTP/1.1 proxy (c=512) | 96,143 RPS | 41,740 RPS | **2.30x** |
| HTTP/2 high concurrency | 100% success | 91.5% success | - |
| Proxy latency overhead | +0.82ms | +1.35ms | **39% lower** |

## Quick Start

```bash
# Build (requires Linux with kernel >= 5.10)
cargo build -p arc-gateway --release

# Run with example config
./target/release/arc-gateway --config arc.yaml
```

Arc supports `.yaml`, `.yml`, `.json`, and `.toml` config files.

Minimal configuration (`arc.yaml`):

```yaml
listen: "0.0.0.0:8080"
admin_listen: "127.0.0.1:9900"

upstreams:
  - name: backend
    addr: "127.0.0.1:3000"

routes:
  - path: /
    upstream: backend
```

Verify it works:

```bash
curl http://localhost:8080/
curl http://localhost:9900/healthz
curl http://localhost:9900/metrics
```

## Features

- **io_uring data plane** - Zero-copy I/O with fixed buffers, multishot accept, kernel-bypass polling
- **Thread-per-core** - Each worker binds to a CPU core with independent resources, no cross-thread contention
- **Radix tree routing** - O(log n) path matching with prefix wildcards, named parameters, and glob patterns
- **Lock-free rate limiting** - GCRA algorithm with atomic CAS, no mutex in hot path
- **WASM plugins** - Wasmtime-based plugin system with instance pooling and epoch-based timeout isolation
- **Hot reload** - ArcSwap-based config updates without dropping connections
- **TLS termination** - Rustls with SNI-based certificate selection and ACME automation
- **HTTP/2 support** - Full h2 multiplexing with configurable stream limits
- **Distributed rate limiting** - Redis-backed global limits with circuit breaker fallback
- **Custom error pages** - Route-level and global error pages with template variables and upstream fallback
- **Multi-format config** - JSON, TOML, and YAML configuration files with hot reload
- **Observability** - Prometheus metrics, phase-level latency tracking, connection lifecycle counters
- **Access logging** - Node-local NDJSON logs with `arc logs tail/query` for local inspection

## Documentation

- [Getting Started](docs/getting-started.md) - Installation, first config, verification
- [Configuration Reference](docs/configuration.md) - Complete field reference with examples
- [Architecture](docs/architecture.md) - Design decisions, module boundaries, io_uring strategy
- [Components](docs/components.md) - Custom vs. borrowed code breakdown
- [Security](docs/security.md) - Request smuggling, slowloris, HPACK bomb defenses
- [Benchmarks](docs/benchmarks.md) - Methodology, environment setup, detailed results
- [Migration from Nginx](docs/migration-from-nginx.md) - Config translation guide

## Requirements

- Linux kernel >= 5.10 (6.1+ recommended for best io_uring support)
- Rust 1.75+
- For distributed rate limiting: Redis 6+

## License

Apache 2.0
