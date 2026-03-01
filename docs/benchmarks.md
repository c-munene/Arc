# Benchmarks

This document describes Arc's benchmark methodology and presents results from controlled tests.

## Environment

All benchmarks were run on:

- **Platform**: WSL2 Debian
- **CPU**: 2 vCPU (virtualized)
- **Kernel**: Linux 5.15+ with io_uring support
- **Network**: Loopback (127.0.0.1)

This is a constrained environment. On bare metal with more cores and a real NIC, absolute numbers will be higher, but the relative comparisons remain valid.

## Methodology

### Tools

- **wrk**: HTTP benchmarking tool for throughput tests
- **h2load**: HTTP/2 benchmarking tool from nghttp2
- **curl**: Verification and latency spot-checks

### Test Configurations

Arc and Nginx were configured as similarly as possible:

- Same number of worker processes/threads
- Same upstream backend
- Keepalive enabled on both downstream and upstream
- No TLS (unless specifically testing TLS)
- No rate limiting (unless specifically testing rate limiting)

### Backend

For throughput tests, we used Nginx serving a static 64-byte response. This isolates proxy overhead from backend processing time.

For latency tests, we used a Python backend with configurable delays to simulate real application behavior.

## Results

### HTTP/1.1 Proxy Throughput

Full feature set enabled: routing, rate limiting (high limit), plugin chain (no-op plugin).

| Concurrency | Arc RPS | Nginx RPS | Ratio |
|-------------|---------|-----------|-------|
| c=256 | 102,203 | 49,930 | **2.05x** |
| c=512 | 96,143 | 41,740 | **2.30x** |

Arc maintains higher throughput as concurrency increases. Nginx's performance degrades more steeply under load.

Command used:
```bash
wrk -t4 -c256 -d30s http://127.0.0.1:8080/
wrk -t4 -c512 -d30s http://127.0.0.1:8080/
```

### HTTP/2 Multiplexing

Testing with multiple streams per connection:

| Test | Arc | Nginx |
|------|-----|-------|
| n=6000, c=24, m=8 streams | 7,200 RPS | 6,984 RPS |
| Ratio | **1.03x** | baseline |

Command used:
```bash
h2load -n6000 -c24 -m8 https://127.0.0.1:8443/
```

### HTTP/2 Stability Under Load

High concurrency stress test:

| Metric | Arc | Nginx |
|--------|-----|-------|
| Requests | 20,000 | 20,000 |
| Concurrency | 32 | 32 |
| Streams/conn | 10 | 10 |
| Success rate | **100%** | 91.5% |

Arc completed all requests successfully. Nginx dropped approximately 8.5% of requests under the same load.

### HTTP/2 Extreme Load

Pushing both proxies to their limits:

| Metric | Arc | Nginx | Ratio |
|--------|-----|-------|-------|
| Requests | 40,000 | 40,000 | - |
| Concurrency | 64 | 64 | - |
| Streams/conn | 20 | 20 | - |
| Throughput | 22,596 RPS | 14,529 RPS | **1.55x** |

### TLS Termination

HTTPS with TLS 1.3, keepalive connections:

| Metric | Arc | Nginx | Ratio |
|--------|-----|-------|-------|
| Throughput (c=128) | ~207 RPS | ~227 RPS | 0.91x |

Nginx has a slight edge in this test because wrk reuses connections aggressively, which masks TLS handshake costs. Arc uses Rustls while Nginx uses OpenSSL; both are well-optimized for the steady-state case.

In short-connection scenarios where handshake overhead matters more, Arc's Rustls implementation performs comparably.

### Proxy Latency Overhead

Measuring the latency added by the proxy layer with a fast backend:

| Layer | Latency |
|-------|---------|
| Direct to backend | 1.18ms |
| Through Nginx | +1.35ms |
| Through Arc | +0.82ms |

Arc adds **39% less latency** than Nginx in the proxy path.

### Arc vs Pingora

Comparing Arc with Cloudflare's Pingora (minimal configuration, same backend):

| Scenario | Arc | Pingora-min | Ratio |
|----------|-----|-------------|-------|
| fast backend c=256 | 102,203 RPS | 30,257 RPS | **3.38x** |
| fast backend c=512 | 96,143 RPS | 27,257 RPS | **3.53x** |
| Proxy latency overhead | +0.82ms | +3.44ms | **Arc 76% lower** |

Pingora's 3.44ms structural latency is a framework ceiling, not a code quality issue. The overhead comes from:

- `async_trait`'s `Box<dyn Future>` requiring heap allocation per request
- Tokio's epoll-based scheduling overhead

These are inherent to the async runtime model and cannot be eliminated without architectural changes.

io_uring's advantage is syscall batching: multiple I/O operations are combined into a single kernel transition. Under high concurrency, this benefit scales linearly with load.

### Distributed Rate Limiting

Testing the Redis-backed global rate limiter with two Arc nodes:

| Scenario | Result |
|----------|--------|
| Uniform traffic (80+80 RPS, limit 100) | 95.80 RPS achieved, limit respected |
| Burst traffic (200+0 RPS, limit 100) | 101.73 RPS achieved, near-optimal |
| Redis failure | Automatic fallback to local limiting |
| Redis recovery | Automatic return to distributed limiting |

The rate limiter correctly enforces global limits across nodes and gracefully handles backend failures.

## Running Your Own Benchmarks

### Setup

1. Build Arc in release mode:
```bash
cargo build -p arc-gateway --release
```

2. Create a minimal config (`bench.json`):
```json
{
  "listen": "0.0.0.0:8080",
  "admin_listen": "127.0.0.1:9900",
  "workers": 0,
  "upstreams": [
    { "name": "backend", "addr": "127.0.0.1:9000" }
  ],
  "routes": [
    { "path": "/", "upstream": "backend" }
  ]
}
```

3. Start a fast backend (Nginx serving static content):
```nginx
# /etc/nginx/nginx.conf
worker_processes auto;
events { worker_connections 4096; }
http {
    server {
        listen 9000;
        location / {
            return 200 'ok';
            add_header Content-Type text/plain;
        }
    }
}
```

4. Start Arc:
```bash
./target/release/arc-gateway --config bench.json
```

### Throughput Test

```bash
# Warm up
wrk -t4 -c128 -d10s http://127.0.0.1:8080/

# Measure
wrk -t4 -c256 -d30s --latency http://127.0.0.1:8080/
```

### Latency Test

For latency measurements, use a constant-rate tool to avoid coordinated omission:

```bash
wrk2 -t4 -c64 -d60s -R10000 --latency http://127.0.0.1:8080/
```

### Comparing with Nginx

Configure Nginx as a reverse proxy:

```nginx
worker_processes auto;
events { worker_connections 4096; }
http {
    upstream backend {
        server 127.0.0.1:9000;
        keepalive 256;
    }
    server {
        listen 8081;
        location / {
            proxy_pass http://backend;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
        }
    }
}
```

Run the same wrk command against port 8081 and compare results.

## Interpreting Results

### What to Look For

- **Requests/sec**: Raw throughput
- **Latency distribution**: p50, p99, max latency
- **Errors**: Any non-2xx responses or socket errors
- **CPU usage**: `top` or `htop` during the test

### Common Issues

**Low throughput**: Check that you're running release builds. Debug builds are 10-50x slower.

**High latency variance**: Ensure nothing else is running on the test machine. Disable CPU frequency scaling if possible.

**Socket errors**: Increase file descriptor limits (`ulimit -n 65536`) and check `net.core.somaxconn`.

**Ring overflow**: If you see `ring_cq_overflow` in metrics, increase `io_uring.entries` in your config.

## Performance Tuning

### Arc Configuration

For maximum throughput:

```json
{
  "io_uring": {
    "entries": 8192,
    "sqpoll": true,
    "sqpoll_idle_ms": 0
  },
  "buffers": {
    "buf_size": 16384,
    "buf_count": 0
  }
}
```

### System Tuning

```bash
# Increase file descriptor limit
ulimit -n 65536

# Increase connection backlog
sysctl -w net.core.somaxconn=65535

# Increase local port range
sysctl -w net.ipv4.ip_local_port_range="1024 65535"

# Enable TCP reuse
sysctl -w net.ipv4.tcp_tw_reuse=1
```

### CPU Affinity

Arc automatically binds workers to CPU cores. For best results, ensure the number of workers matches your physical core count (not hyperthreads).

```json
{
  "workers": 4
}
```

Set this explicitly rather than using `0` (auto-detect) if you want consistent benchmark results.
