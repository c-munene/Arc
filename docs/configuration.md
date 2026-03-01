# Configuration Reference

Arc supports JSON, TOML, and YAML configuration files. The format is detected by file extension (`.json`, `.toml`, `.yaml`, `.yml`). Config files are watched for changes and hot-reloaded without dropping connections.

When using TOML or YAML, Arc internally converts the configuration to canonical JSON for consistency. The control plane API continues to use JSON only.

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `listen` | string | `"0.0.0.0:8080"` | Address for the data plane listener |
| `admin_listen` | string | `"127.0.0.1:9900"` | Address for metrics and health endpoints |
| `workers` | integer | `0` | Number of worker threads. `0` means auto-detect CPU count |
| `linger_ms` | integer | `300` | TCP linger timeout in milliseconds |

## io_uring Settings

The `io_uring` object controls kernel I/O behavior:

```yaml
io_uring:
  entries: 4096
  accept_multishot: true
  tick_ms: 10
  sqpoll: true
  sqpoll_idle_ms: 0
  iopoll: false
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `entries` | integer | `4096` | Size of the submission/completion rings |
| `accept_multishot` | boolean | `true` | Use multishot accept if kernel supports it |
| `tick_ms` | integer | `10` | Interval for timeout scanning and config checks |
| `sqpoll` | boolean | `true` | Enable kernel-side submission queue polling |
| `sqpoll_idle_ms` | integer | `0` | SQPOLL idle timeout. `0` means never idle |
| `iopoll` | boolean | `false` | Enable busy-polling for completions |

For most workloads, the defaults work well. Increase `entries` if you see `ring_sq_dropped` in metrics.

## Buffer Settings

```yaml
buffers:
  buf_size: 8192
  buf_count: 0
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `buf_size` | integer | `8192` | Size of each fixed buffer in bytes |
| `buf_count` | integer | `0` | Number of buffers per worker. `0` means auto-calculate |

The buffer size must be large enough to hold complete HTTP headers. If you see parsing errors with large headers, increase `buf_size`.

## Timeouts

```yaml
timeouts_ms:
  cli_handshake: 3000
  cli_read: 30000
  cli_write: 30000
  up_conn: 3000
  up_handshake: 3000
  up_read: 30000
  up_write: 30000
```

| Field | Description |
|-------|-------------|
| `cli_handshake` | TLS handshake timeout for downstream connections |
| `cli_read` | Timeout waiting for client request data |
| `cli_write` | Timeout writing response to client |
| `up_conn` | Timeout establishing upstream connection |
| `up_handshake` | TLS handshake timeout for upstream connections |
| `up_read` | Timeout reading upstream response |
| `up_write` | Timeout writing request to upstream |

All values are in milliseconds.

## Upstreams

Define backend servers in the `upstreams` array:

```yaml
upstreams:
  - name: api
    addr: "127.0.0.1:3000"
    keepalive: 1024
    idle_ttl_ms: 30000
  - name: static
    addr: "127.0.0.1:3001"
    keepalive: 256
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Unique identifier referenced by routes |
| `addr` | string | required | Backend address as `host:port` |
| `keepalive` | integer | `128` | Max idle connections to keep in pool |
| `idle_ttl_ms` | integer | `30000` | How long to keep idle connections |

## Routes

Routes map request paths to upstreams:

```yaml
routes:
  - path: /api/v1/*
    upstream: api
    plugins: [auth]
    rate_limit:
      rps: 1000
      burst: 2000
  - path: /static/*
    upstream: static
  - path: /
    upstream: api
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | required | Path pattern to match |
| `upstream` | string | required | Name of upstream to forward to |
| `plugins` | array | `[]` | List of plugin names to execute |
| `rate_limit` | object | `null` | Per-route rate limiting |

### Path Patterns

Arc supports several path pattern types:

- **Exact**: `/api/v1/users` matches only that exact path
- **Prefix wildcard**: `/static/*` matches `/static`, `/static/`, `/static/foo/bar`
- **Named parameters**: `/users/:id/profile` captures the `id` segment
- **Segment wildcards**: `/foo/*/bar` matches any single segment in the middle
- **Globs**: `/assets/*.css` matches files ending in `.css`

Routes are matched by specificity. Exact matches win over prefix matches, and longer patterns win over shorter ones.

### Rate Limiting

```yaml
rate_limit:
  rps: 100
  burst: 200
```

| Field | Type | Description |
|-------|------|-------------|
| `rps` | integer | Requests per second limit |
| `burst` | integer | Maximum burst size (token bucket capacity) |

The rate limiter uses the GCRA algorithm with atomic operations, so there's no lock contention in the hot path.

## Plugins

WASM plugins are defined globally and referenced by routes:

```yaml
plugins:
  - name: auth
    path: plugins/auth.wasm
    pool: 8
    timeout_ms: 2
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Identifier used in route `plugins` arrays |
| `path` | string | required | Path to the `.wasm` file |
| `pool` | integer | `8` | Number of pre-instantiated WASM instances |
| `timeout_ms` | integer | `2` | Execution timeout per invocation |

Plugins must export specific functions. See the plugin development guide for the ABI specification.

## TLS Configuration

### Downstream TLS (Client-facing)

```yaml
downstream_tls:
  enable_h2: true
  certificates:
    - sni: example.com
      cert_pem: ./certs/example.com.crt
      key_pem: ./certs/example.com.key
    - sni: "*.example.com"
      cert_pem: ./certs/wildcard.crt
      key_pem: ./certs/wildcard.key
      wildcard: true
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable_h2` | boolean | `false` | Enable HTTP/2 via ALPN negotiation |
| `certificates` | array | required | List of certificate configurations |

Certificate fields:

| Field | Type | Description |
|-------|------|-------------|
| `sni` | string | Server name for certificate selection |
| `cert_pem` | string | Path to certificate file (PEM format) |
| `key_pem` | string | Path to private key file (PEM format) |
| `wildcard` | boolean | Whether this is a wildcard certificate |

The first certificate in the list is used as the default when no SNI matches.

### Upstream TLS (Backend connections)

```yaml
upstreams:
  - name: secure-backend
    addr: "backend.internal:443"
    tls:
      server_name: backend.internal
      ca_pem: ./certs/ca.crt
      client_cert_pem: ./certs/client.crt
      client_key_pem: ./certs/client.key
      insecure_skip_verify: false
      enable_resumption: true
```

## ACME (Automatic Certificates)

Arc can automatically obtain and renew certificates from Let's Encrypt:

```yaml
acme:
  enabled: true
  email: admin@example.com
  domains:
    - example.com
    - www.example.com
  staging: false
  challenge: tls-alpn-01
  cache_dir: ./acme_cache
  master_key: "base64:your-32-byte-key-here"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable ACME certificate management |
| `email` | string | required | Contact email for Let's Encrypt |
| `domains` | array | required | Domains to obtain certificates for |
| `staging` | boolean | `true` | Use Let's Encrypt staging environment |
| `challenge` | string | `"tls-alpn-01"` | Challenge type: `tls-alpn-01` or `http-01` |
| `cache_dir` | string | `"./acme_cache"` | Directory for encrypted certificate storage |
| `master_key` | string | required | Encryption key for cached certificates |

For `http-01` challenges, you must also set `http01_listen` to a port 80 listener address.

## Global Rate Limiting

For distributed deployments, Arc supports Redis-backed rate limiting:

```yaml
global_rate_limit:
  backend: redis
  redis:
    url: redis://127.0.0.1:6379/0
    budget_ms: 2
    circuit_open_ms: 500
    prefetch: 128
    low_watermark: 16
    refill_backoff_ms: 1
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `backend` | string | `"in_memory"` | Backend type: `in_memory` or `redis` |
| `url` | string | required for redis | Redis connection URL |
| `budget_ms` | integer | `2` | Max time to wait for Redis response |
| `circuit_open_ms` | integer | `500` | How long to skip Redis after failures |
| `prefetch` | integer | `128` | Tokens to request per batch |
| `low_watermark` | integer | `16` | Trigger refill when tokens drop below this |

When Redis is unavailable, Arc automatically falls back to local rate limiting with no request failures.

## Control Plane

For multi-node deployments with config synchronization:

```yaml
control_plane:
  enabled: true
  bind: "127.0.0.1:19998"
  role: leader
  node_id: arc-node-1
  peers:
    - http://arc-node-2:19998
    - http://arc-node-3:19998
  auth_token: secret-token
  quorum: 2
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable control plane |
| `bind` | string | `"127.0.0.1:19998"` | Control plane listen address |
| `role` | string | `"standalone"` | Role: `standalone`, `leader`, or `follower` |
| `node_id` | string | required | Unique identifier for this node |
| `peers` | array | `[]` | URLs of peer nodes |
| `auth_token` | string | `null` | Bearer token for API authentication |
| `quorum` | integer | `0` | Minimum nodes for config commit. `0` means majority |

Leaders push config changes to followers. Followers long-poll the leader for updates.

## Error Pages

Arc supports custom error pages at both global and route levels. Route-level error pages take priority over global defaults.

### Global Defaults

```yaml
defaults:
  error_pages:
    "502":
      body: "<h1>Bad Gateway</h1><p>Request ID: $request_id</p>"
      content_type: text/html
    5xx:
      file: ./error_pages/5xx.html
      content_type: text/html
```

### Route-Level Error Pages

```yaml
routes:
  - path: /api/*
    upstream: api
    error_pages:
      502-504:
        when: upstream_error
        redirect: /maintenance
        code: 302
      "500":
        upstream: fallback
```

### Pattern Syntax

Error page keys support three pattern types:

| Pattern | Example | Description |
|---------|---------|-------------|
| Exact | `502` | Matches only status code 502 |
| Range | `502-504` | Matches 502, 503, and 504 |
| Class | `5xx` | Matches all 5xx status codes (500-599) |

Patterns are matched by specificity: exact codes first, then ranges (narrower ranges win), then classes.

### When Condition

| Value | Description |
|-------|-------------|
| `any` | Trigger on any matching status code (default) |
| `upstream_error` | Only trigger when the error originated from upstream |

### Actions

Each error page must specify exactly one action:

| Field | Type | Description |
|-------|------|-------------|
| `body` | string | Inline response body (supports template variables) |
| `file` | string | Path to file containing response body |
| `redirect` | string | URL to redirect to (requires `code` field) |
| `upstream` | string | Name of upstream to forward the request to |

Additional fields:

| Field | Type | Description |
|-------|------|-------------|
| `content_type` | string | Content-Type header (required for `body` and `file`) |
| `code` | integer | Redirect status code (300-399, required for `redirect`) |

### Template Variables

The `body` and `file` actions support template variables that are expanded at runtime:

| Variable | Description |
|----------|-------------|
| `$request_id` | Unique identifier for the request |
| `$upstream.name` | Name of the upstream that generated the error |
| `$error.status` | HTTP status code |
| `$error.source` | Error source (e.g., "upstream", "timeout") |
| `$route.id` | Numeric route identifier |

Example template:

```html
<!DOCTYPE html>
<html>
<head><title>Error $error.status</title></head>
<body>
  <h1>Something went wrong</h1>
  <p>Status: $error.status</p>
  <p>Request ID: $request_id</p>
  <p>Upstream: $upstream.name</p>
</body>
</html>
```

### Hot Reload

Error page files referenced by `file` actions are included in the hot reload fingerprint. When these files change on disk, Arc automatically reloads the configuration.

## Access Logging

Arc access logs are written to local files on each node. The logging fields below are consumed by `arc-logging`.

```yaml
logging:
  output:
    file: /var/log/arc/access.log
    stdout: false
    rotation:
      max_size: 1gb
      max_files: 7
      compress: true
  access:
    sample: 0.01
    force_on_status: [401, 403, 429, 500, 502, 503, 504]
    force_on_slow: 500ms
```

### Access Field Semantics

| Field | Type | Default | Effective Condition | Notes |
|-------|------|---------|---------------------|-------|
| `logging.access.sample` | float `[0,1]` | `0.01` | Always | Normal request sampling rate |
| `logging.access.force_on_status` | array of HTTP status codes | `[401,403,429,500,502,503,504]` | Response status in list | Explicit list only; Arc does not infer status classes |
| `logging.access.force_on_slow` | duration/ms | `500ms` | `duration_ms > force_on_slow` | `0` disables slow-force |
| `logging.output.rotation.max_size` | size string/bytes | `1gb` | file size reaches threshold | Triggers rotate |
| `logging.output.rotation.max_files` | integer | `7` | Always | Retention count for rotated archives |
| `logging.output.rotation.compress` | bool | `true` | Rotate after rename | Compression runs in dedicated background thread |

### Rotation and Compression Pipeline

When rotation is triggered:

1. Arc renames the current log file to a timestamped archive name.
2. Arc immediately opens a new active log file and continues writing.
3. If `compress: true`, gzip compression is queued to a dedicated background thread.
4. Old archives beyond `max_files` are cleaned up.

This keeps the write path isolated from compression latency.

### Trace Context Rules

Arc follows W3C `traceparent` semantics for log context:

1. If inbound request carries valid `traceparent`, Arc parses `trace_id` and `span_id` from it.
2. If inbound request has no valid `traceparent`, Arc generates a new 128-bit `trace_id` and 64-bit `span_id`.
3. The resolved IDs are stored in request context and written to access logs.
4. Forwarding should propagate `traceparent` from request context (typically using a child span for upstream).

If your existing observability pipeline already owns trace context, logging should read from that context and not duplicate generation in hot path.

### Local vs Cluster Scope

- Arc writes access logs per node, to local files.
- `arc logs tail` and `arc logs query` are local node commands.
- Cluster-wide log aggregation is out of Arc scope. Use external collectors such as Vector or Fluent Bit, with backends such as Loki or Elasticsearch.

### CLI Filtering Cost

`arc logs tail` and `arc logs query` filter on the CLI side:

- The CLI reads log lines, parses JSON locally, then applies filters.
- Under high sampling/high QPS, always prefer `--last` to bound scan range.
- If needed, lower sample rate before long-running tail/query sessions.

## Complete Example

```yaml
listen: "0.0.0.0:8443"
admin_listen: "127.0.0.1:9900"
workers: 4

io_uring:
  entries: 4096
  sqpoll: true

timeouts_ms:
  cli_read: 30000
  up_conn: 3000
  up_read: 60000

downstream_tls:
  enable_h2: true
  certificates:
    - sni: api.example.com
      cert_pem: ./certs/api.crt
      key_pem: ./certs/api.key

defaults:
  error_pages:
    5xx:
      body: Service temporarily unavailable
      content_type: text/plain

upstreams:
  - name: api
    addr: "127.0.0.1:3000"
    keepalive: 512

plugins:
  - name: auth
    path: ./plugins/auth.wasm
    pool: 16
    timeout_ms: 5

routes:
  - path: /api/v1/*
    upstream: api
    plugins: [auth]
    rate_limit:
      rps: 10000
      burst: 20000
    error_pages:
      "502":
        when: upstream_error
        redirect: /maintenance
        code: 302
  - path: /health
    upstream: api

global_rate_limit:
  backend: redis
  redis:
    url: redis://localhost:6379/0
```

JSON and TOML formats are also supported. See the examples below for reference.

<details>
<summary>JSON Format</summary>

```json
{
  "listen": "0.0.0.0:8443",
  "admin_listen": "127.0.0.1:9900",
  "workers": 4,
  "io_uring": {
    "entries": 4096,
    "sqpoll": true
  },
  "timeouts_ms": {
    "cli_read": 30000,
    "up_conn": 3000,
    "up_read": 60000
  },
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [
      {
        "sni": "api.example.com",
        "cert_pem": "./certs/api.crt",
        "key_pem": "./certs/api.key"
      }
    ]
  },
  "defaults": {
    "error_pages": {
      "5xx": {
        "body": "Service temporarily unavailable",
        "content_type": "text/plain"
      }
    }
  },
  "upstreams": [
    {
      "name": "api",
      "addr": "127.0.0.1:3000",
      "keepalive": 512
    }
  ],
  "plugins": [
    {
      "name": "auth",
      "path": "./plugins/auth.wasm",
      "pool": 16,
      "timeout_ms": 5
    }
  ],
  "routes": [
    {
      "path": "/api/v1/*",
      "upstream": "api",
      "plugins": ["auth"],
      "rate_limit": { "rps": 10000, "burst": 20000 },
      "error_pages": {
        "502": {
          "when": "upstream_error",
          "redirect": "/maintenance",
          "code": 302
        }
      }
    },
    {
      "path": "/health",
      "upstream": "api"
    }
  ],
  "global_rate_limit": {
    "backend": "redis",
    "redis": {
      "url": "redis://localhost:6379/0"
    }
  }
}
```

</details>

<details>
<summary>TOML Format</summary>

```toml
listen = "0.0.0.0:8443"
admin_listen = "127.0.0.1:9900"
workers = 4

[io_uring]
entries = 4096
sqpoll = true

[timeouts_ms]
cli_read = 30000
up_conn = 3000
up_read = 60000

[downstream_tls]
enable_h2 = true

[[downstream_tls.certificates]]
sni = "api.example.com"
cert_pem = "./certs/api.crt"
key_pem = "./certs/api.key"

[defaults.error_pages.5xx]
body = "Service temporarily unavailable"
content_type = "text/plain"

[[upstreams]]
name = "api"
addr = "127.0.0.1:3000"
keepalive = 512

[[plugins]]
name = "auth"
path = "./plugins/auth.wasm"
pool = 16
timeout_ms = 5

[[routes]]
path = "/api/v1/*"
upstream = "api"
plugins = ["auth"]

[routes.rate_limit]
rps = 10000
burst = 20000

[routes.error_pages.502]
when = "upstream_error"
redirect = "/maintenance"
code = 302

[[routes]]
path = "/health"
upstream = "api"

[global_rate_limit]
backend = "redis"

[global_rate_limit.redis]
url = "redis://localhost:6379/0"
```

</details>
