#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/config_format_runtime"
mkdir -p "$WORK"

cat > "$WORK/arc.json" <<'EOF'
{
  "listen": "127.0.0.1:18087",
  "admin_listen": "127.0.0.1:19097",
  "workers": 1,
  "require_upstream_mtls": false,
  "io_uring": {
    "entries": 256,
    "accept_multishot": false,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "buffers": { "buf_size": 8192, "buf_count": 256 },
  "timeouts_ms": {
    "cli_read": 1000,
    "up_conn": 1000,
    "up_write": 1000,
    "up_read": 1000,
    "cli_write": 1000
  },
  "upstreams": [
    { "name": "u", "addr": "127.0.0.1:19080", "keepalive": 8, "idle_ttl_ms": 1000 }
  ],
  "plugins": [],
  "routes": [
    { "path": "/", "upstream": "u" }
  ]
}
EOF

cat > "$WORK/arc.toml" <<'EOF'
listen = "127.0.0.1:18088"
admin_listen = "127.0.0.1:19098"
workers = 1
require_upstream_mtls = false
plugins = []

[io_uring]
entries = 256
accept_multishot = false
tick_ms = 10
sqpoll = false
sqpoll_idle_ms = 0
iopoll = false

[buffers]
buf_size = 8192
buf_count = 256

[timeouts_ms]
cli_read = 1000
up_conn = 1000
up_write = 1000
up_read = 1000
cli_write = 1000

[[upstreams]]
name = "u"
addr = "127.0.0.1:19080"
keepalive = 8
idle_ttl_ms = 1000

[[routes]]
path = "/"
upstream = "u"
EOF

cat > "$WORK/arc.yaml" <<'EOF'
listen: "127.0.0.1:18089"
admin_listen: "127.0.0.1:19099"
workers: 1
require_upstream_mtls: false
io_uring:
  entries: 256
  accept_multishot: false
  tick_ms: 10
  sqpoll: false
  sqpoll_idle_ms: 0
  iopoll: false
buffers:
  buf_size: 8192
  buf_count: 256
timeouts_ms:
  cli_read: 1000
  up_conn: 1000
  up_write: 1000
  up_read: 1000
  cli_write: 1000
upstreams:
  - name: "u"
    addr: "127.0.0.1:19080"
    keepalive: 8
    idle_ttl_ms: 1000
plugins: []
routes:
  - path: "/"
    upstream: "u"
EOF

cd "$ROOT"
cargo build -p arc-gateway >/dev/null

check_one() {
  local cfg="$1"
  local log="$2"
  target/debug/arc-gateway --config "$cfg" >"$log" 2>&1 &
  local pid=$!
  sleep 1
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "FAIL $cfg"
    tail -n 120 "$log" || true
    return 1
  fi
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  echo "OK $cfg"
}

check_one "$WORK/arc.json" "$WORK/arc_json.log"
check_one "$WORK/arc.toml" "$WORK/arc_toml.log"
check_one "$WORK/arc.yaml" "$WORK/arc_yaml.log"
