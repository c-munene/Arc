# Getting Started

This guide walks you through building Arc, writing your first configuration, and verifying everything works.

## Prerequisites

Arc requires Linux with io_uring support. The minimum kernel version is 5.10, but 6.1 or later is recommended for features like multishot accept and SQPOLL.

Check your kernel version:

```bash
uname -r
```

You'll also need Rust 1.75 or later:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
```

## Building

Clone the repository and build in release mode:

```bash
git clone https://github.com/your-org/arc.git
cd arc
cargo build -p arc-gateway --release
```

The binary will be at `./target/release/arc-gateway`.

For development builds with debug symbols:

```bash
cargo build -p arc-gateway
```

## Your First Configuration

Create a file called `arc.yaml`:

```yaml
listen: "0.0.0.0:8080"
admin_listen: "127.0.0.1:9900"
workers: 0  # auto-detect CPU cores

upstreams:
  - name: backend
    addr: "127.0.0.1:3000"
    keepalive: 128
    idle_ttl_ms: 30000

routes:
  - path: /api/*
    upstream: backend
  - path: /
    upstream: backend
```

Arc also supports JSON and TOML formats if you prefer those.

## Starting Arc

Start a simple backend for testing (Python example):

```bash
python3 -m http.server 3000 &
```

Or use any HTTP server you have available on port 3000.

Now start Arc:

```bash
./target/release/arc-gateway --config arc.yaml
```

You should see output indicating the workers have started.

## Verification

Test that requests are being proxied:

```bash
curl -i http://localhost:8080/
```

Check the health endpoint:

```bash
curl http://localhost:9900/healthz
```

Expected response: `ok`

View Prometheus metrics:

```bash
curl http://localhost:9900/metrics
```

You'll see counters like `arc_requests_total`, `arc_active_current`, and phase timing metrics.

## Local Access Log Query

`arc logs` commands read local log files on the current node:

```bash
arc logs tail --last 5m
arc logs query --status 5xx --last 10m
```

For cluster-wide log search, use external collection and aggregation (for example, Vector/Fluent Bit + Loki/Elasticsearch).

Filtering is done in the CLI process. On high sampling or high QPS workloads, always include `--last` to keep scan cost bounded.

## Adding Rate Limiting

Update your config to add per-route rate limits:

```yaml
listen: "0.0.0.0:8080"
admin_listen: "127.0.0.1:9900"

upstreams:
  - name: backend
    addr: "127.0.0.1:3000"

routes:
  - path: /api/*
    upstream: backend
    rate_limit:
      rps: 100
      burst: 200
  - path: /
    upstream: backend
```

Arc watches the config file for changes. After saving, the new rate limits take effect within 500ms without dropping connections.

## Adding TLS

To enable HTTPS, add a `downstream_tls` section:

```yaml
listen: "0.0.0.0:8443"
admin_listen: "127.0.0.1:9900"

downstream_tls:
  certificates:
    - sni: example.com
      cert_pem: ./certs/example.com.crt
      key_pem: ./certs/example.com.key

upstreams:
  - name: backend
    addr: "127.0.0.1:3000"

routes:
  - path: /
    upstream: backend
```

Arc uses Rustls for TLS termination. Certificate files are read at startup and on hot reload.

## Next Steps

- Read the [Configuration Reference](configuration.md) for all available options
- See [Architecture](architecture.md) to understand how Arc achieves its performance
- Check [Migration from Nginx](migration-from-nginx.md) if you're coming from an Nginx setup
