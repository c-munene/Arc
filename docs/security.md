# Security

Arc is designed to be a secure-by-default reverse proxy. This document describes the security measures implemented and how to configure Arc for defense in depth.

## HTTP/1.1 Request Smuggling

HTTP Request Smuggling exploits ambiguity between how a proxy and backend interpret message boundaries. The classic attack uses conflicting `Content-Length` and `Transfer-Encoding` headers.

### Arc's Approach

Arc uses a strict, unambiguous parsing strategy:

```
Transfer-Encoding: chunked  →  always wins (per RFC 7230)
Content-Length only         →  use Content-Length
Neither                     →  no body (for requests)
```

Key implementation details (`arc-proto-http1`):

1. **Single-pass header scan**: Arc scans headers once, tracking both `Content-Length` and `Transfer-Encoding: chunked`. If chunked is present, Content-Length is ignored entirely.

2. **No CL+TE ambiguity**: Arc never forwards both headers to upstream. The body framing decision is made once at parse time.

3. **Strict chunked parsing**: The chunked decoder is a state machine that validates chunk sizes and terminators. Malformed chunks cause connection close, not silent truncation.

4. **No request pipelining reuse on ambiguity**: If a request's body boundary is unclear, Arc closes the connection rather than risk desync.

```rust
// From arc-proto-http1/src/lib.rs
let body = if transfer_chunked {
    BodyKind::Chunked(ChunkedState::new())  // TE wins
} else if let Some(cl) = content_length {
    BodyKind::ContentLength { remaining: cl }
} else {
    BodyKind::None
};
```

### What Arc Does NOT Do

- Arc does not attempt to "normalize" conflicting headers
- Arc does not strip one header and forward the other
- Arc does not guess intent from header ordering

This eliminates CL.TE, TE.CL, and TE.TE smuggling variants.

## Slowloris and Slow Read Attacks

Slowloris attacks exhaust server resources by opening many connections and sending data very slowly, keeping connections alive indefinitely.

### Arc's Defenses

1. **Per-phase timeouts**: Every connection phase has an independent timeout:

```yaml
timeouts_ms:
  cli_handshake: 3000   # TLS handshake
  cli_read: 30000       # Reading client request
  cli_write: 30000      # Writing response to client
  up_conn: 3000         # Connecting to upstream
  up_handshake: 3000    # Upstream TLS handshake
  up_read: 30000        # Reading upstream response
  up_write: 30000       # Writing to upstream
```

2. **Timeout wheel**: Arc uses a hierarchical timing wheel for O(1) timeout insertion and efficient batch expiration. Every active connection is tracked.

3. **Phase transitions reset deadlines**: When a connection moves to a new phase (e.g., from reading headers to writing to upstream), the deadline is reset. A slow client can't accumulate time across phases.

4. **No infinite waits**: There is no code path where Arc waits indefinitely for client data. Even idle keepalive connections have TTL enforcement.

5. **Fixed buffer architecture**: Each connection uses a fixed-size buffer from a pre-allocated pool. A slow client cannot cause unbounded memory growth.

### Tuning for Your Environment

For public-facing deployments with untrusted clients:

```yaml
timeouts_ms:
  cli_handshake: 5000   # Allow slow TLS on mobile
  cli_read: 10000       # 10s to send request (reduce from default)
  cli_write: 30000      # Response writes can be slower
```

For internal services with trusted clients:

```yaml
timeouts_ms:
  cli_read: 60000       # Allow longer for large uploads
  up_read: 120000       # Allow slow backend processing
```

## HTTP/2 HPACK Bomb

HPACK compression uses a dynamic table that can be exploited to amplify small inputs into large memory allocations. An attacker sends headers that expand dramatically when decompressed.

### Arc's Defenses

1. **Header block size limit**: Before HPACK decoding, Arc checks the compressed block size:

```rust
// From arc-proto-h2/src/hpack.rs
if block.len() > self.limits.max_header_block_bytes {
    return Err(H2Error::new(H2Code::FrameSizeError, "header block too large"));
}
```

Default: 64KB maximum compressed header block.

2. **Header count limit**: After decoding, Arc limits the number of headers:

```rust
if headers.len() > self.limits.max_headers {
    return Err(H2Error::new(H2Code::ProtocolError, "too many headers in block"));
}
```

Default: 256 headers maximum per request.

3. **Dynamic table size limit**: The HPACK dynamic table size is bounded by `SETTINGS_HEADER_TABLE_SIZE` (default 4KB). Arc advertises this limit and rejects peers that exceed it.

4. **Frame size enforcement**: Individual frames are limited by `SETTINGS_MAX_FRAME_SIZE`:

```rust
// From arc-proto-h2/src/frame.rs
if len > max_frame_size.min(16_777_215) {
    return Err(H2Error::new(H2Code::FrameSizeError, "frame payload exceeds peer max_frame_size"));
}
```

Default: 16KB per frame.

5. **Concurrent stream limits**: `SETTINGS_MAX_CONCURRENT_STREAMS` (default 1024) prevents resource exhaustion from too many simultaneous requests.

### H2 Settings Defaults

```rust
Settings {
    header_table_size: 4096,        // 4KB HPACK dynamic table
    enable_push: true,
    max_concurrent_streams: 1024,   // Max parallel streams
    initial_window_size: 65_535,    // 64KB flow control window
    max_frame_size: 16_384,         // 16KB max frame
    max_header_list_size: 65_536,   // 64KB max headers (uncompressed)
}
```

## TLS Security

Arc uses Rustls, which provides:

- **No OpenSSL**: Rustls is a pure-Rust implementation, eliminating entire classes of memory safety vulnerabilities
- **TLS 1.2 and 1.3 only**: Legacy protocols (SSL 3.0, TLS 1.0, TLS 1.1) are not supported
- **Safe cipher suites**: Only AEAD ciphers (AES-GCM, ChaCha20-Poly1305) are enabled
- **Certificate validation**: Strict chain validation with configurable CA roots

### SNI-Based Routing

Arc selects certificates based on SNI during the TLS handshake. If no SNI matches, the first certificate in the list is used as default. This prevents certificate mismatch errors while maintaining security.

### ACME Security

When using automatic certificate management:

- Private keys are encrypted at rest using the configured `master_key`
- Challenge tokens are validated before certificate issuance
- Certificates are renewed before expiration (default: 30 days before)

## Rate Limiting

Arc provides multiple layers of rate limiting:

### Per-Route Limits

```yaml
routes:
  - path: /api/*
    upstream: api
    rate_limit:
      rps: 100
      burst: 200
```

Uses the GCRA (Generic Cell Rate Algorithm) with atomic operations. No locks in the hot path.

### Global Distributed Limits

For multi-node deployments:

```yaml
global_rate_limit:
  backend: redis
  redis:
    url: redis://localhost:6379/0
    budget_ms: 2          # Max Redis latency
    circuit_open_ms: 500  # Fallback duration on Redis failure
```

The circuit breaker ensures Redis failures don't cascade to request failures.

## Plugin Isolation

WASM plugins run in Wasmtime with multiple isolation layers:

1. **Memory isolation**: Each plugin instance has its own linear memory
2. **Epoch-based timeouts**: Plugins that exceed their timeout are interrupted mid-execution
3. **Panic isolation**: Plugin crashes return 500 errors without affecting the worker
4. **Instance pooling**: Pre-instantiated instances prevent cold-start resource spikes

```yaml
plugins:
  - name: auth
    path: ./plugins/auth.wasm
    pool: 16          # Pre-instantiated instances
    timeout_ms: 5     # Hard execution limit
```

## Upstream Security

### mTLS to Backends

Arc supports mutual TLS for upstream connections:

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
```

### Connection Reuse Safety

Arc tracks upstream connection state to prevent request/response desync:

- Connections are validated before reuse
- Idle connections are monitored via epoll for server-side closes
- TTL enforcement prevents stale connection reuse

## Deployment Recommendations

### Minimal Attack Surface

```yaml
# Bind admin to localhost only
admin_listen: "127.0.0.1:9900"

# Disable unused features
downstream_tls:
  enable_h2: false  # If you don't need HTTP/2
```

### Defense in Depth

1. Run Arc behind a cloud WAF for L7 filtering
2. Use network policies to restrict upstream access
3. Enable mTLS for all internal communication
4. Monitor `/metrics` for anomalies (connection spikes, timeout increases)

### File Descriptor Limits

Ensure adequate limits for high-concurrency deployments:

```bash
ulimit -n 65536
```

### Kernel Hardening

```bash
# Prevent SYN floods
sysctl -w net.ipv4.tcp_syncookies=1

# Reduce TIME_WAIT accumulation
sysctl -w net.ipv4.tcp_tw_reuse=1
```

## Reporting Vulnerabilities

If you discover a security vulnerability in Arc, please report it responsibly:

1. Do not open a public GitHub issue
2. Email security@[project-domain] with details
3. Include steps to reproduce if possible
4. Allow reasonable time for a fix before disclosure

We aim to acknowledge reports within 48 hours and provide fixes within 14 days for critical issues.
