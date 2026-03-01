#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RESULT_ROOT="$ROOT/benchmark/results"
RUN_ID="${RUN_ID:-$(date +%Y%m%d_%H%M%S)}"
OUT_DIR="${OUT_DIR:-$RESULT_ROOT/h1_wrk_${RUN_ID}}"

WRK_BIN="${WRK_BIN:-wrk}"
NGINX_BIN="${NGINX_BIN:-nginx}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
ARC_BIN="${ARC_BIN:-$ROOT/target/release/arc-gateway}"
CURL_BIN="${CURL_BIN:-curl}"

RUNS="${RUNS:-5}"
THREADS="${THREADS:-8}"
CONNECTIONS="${CONNECTIONS:-256}"
DURATION="${DURATION:-30s}"
WARMUP="${WARMUP:-5s}"
REQUIRE_ZERO_NON2XX="${REQUIRE_ZERO_NON2XX:-1}"

ARC_PORT="${ARC_PORT:-18480}"
NGINX_PORT="${NGINX_PORT:-18481}"
BACKEND_PORT="${BACKEND_PORT:-19080}"
ADMIN_PORT="${ADMIN_PORT:-19980}"
ARC_WORKERS="${ARC_WORKERS:-1}"

mkdir -p "$OUT_DIR"

require_bin() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    echo "missing required command: $name"
    exit 1
  fi
}

require_bin "$WRK_BIN"
require_bin "$NGINX_BIN"
require_bin "$PYTHON_BIN"
require_bin "$CURL_BIN"

if [[ ! -x "$ARC_BIN" ]]; then
  echo "missing arc binary: $ARC_BIN"
  echo "hint: cargo build --release -p arc-gateway"
  exit 1
fi

kill_listen_port() {
  local port="$1"
  local pids
  pids="$(
    ss -ltnp 2>/dev/null \
      | awk -v p=":${port}" '
          $4 ~ p {
            if (match($0, /pid=[0-9]+/)) {
              print substr($0, RSTART + 4, RLENGTH - 4)
            }
          }' \
      | sort -u || true
  )"
  if [[ -n "$pids" ]]; then
    # shellcheck disable=SC2086
    kill -9 $pids >/dev/null 2>&1 || true
  fi
}

cleanup() {
  set +e
  "$NGINX_BIN" -p "$OUT_DIR/nginx_runtime" -c "$OUT_DIR/nginx.conf" -s stop >/dev/null 2>&1 || true
  if [[ -f "$OUT_DIR/arc.pid" ]]; then
    kill "$(cat "$OUT_DIR/arc.pid")" >/dev/null 2>&1 || true
  fi
  if [[ -f "$OUT_DIR/backend.pid" ]]; then
    kill "$(cat "$OUT_DIR/backend.pid")" >/dev/null 2>&1 || true
  fi
  wait >/dev/null 2>&1 || true
  kill_listen_port "$ARC_PORT"
  kill_listen_port "$NGINX_PORT"
  kill_listen_port "$BACKEND_PORT"
  kill_listen_port "$ADMIN_PORT"
}
trap cleanup EXIT

mkdir -p "$OUT_DIR/nginx_runtime/logs"
kill_listen_port "$ARC_PORT"
kill_listen_port "$NGINX_PORT"
kill_listen_port "$BACKEND_PORT"
kill_listen_port "$ADMIN_PORT"

cat > "$OUT_DIR/nginx.conf" <<EOF
worker_processes auto;
pid $OUT_DIR/nginx.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log $OUT_DIR/nginx.error.log warn;
  upstream app_backend {
    server 127.0.0.1:${BACKEND_PORT};
    keepalive 1024;
  }
  server {
    listen 127.0.0.1:${NGINX_PORT};
    location / {
      proxy_http_version 1.1;
      proxy_set_header Connection "";
      proxy_set_header Host \$host;
      proxy_pass http://app_backend;
    }
  }
}
EOF

cat > "$OUT_DIR/arc.json" <<EOF
{
  "listen": "127.0.0.1:${ARC_PORT}",
  "admin_listen": "127.0.0.1:${ADMIN_PORT}",
  "workers": ${ARC_WORKERS},
  "linger_ms": 100,
  "io_uring": {
    "entries": 4096,
    "accept_multishot": true,
    "accept_prepost": 32,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "listen_backlog": 4096,
  "buffers": { "buf_size": 8192, "buf_count": 1024 },
  "timeouts_ms": {
    "cli_handshake": 1000,
    "cli_read": 30000,
    "up_conn": 3000,
    "up_handshake": 1000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "require_upstream_mtls": false,
  "upstreams": [
    {
      "name": "u",
      "addr": "127.0.0.1:${BACKEND_PORT}",
      "keepalive": 1024,
      "idle_ttl_ms": 30000
    }
  ],
  "plugins": [],
  "routes": [
    { "path": "/x", "upstream": "u", "plugins": [], "rate_limit": null }
  ]
}
EOF

"$PYTHON_BIN" "$ROOT/benchmark/backends/http_ok_backend.py" \
  --port "$BACKEND_PORT" \
  --name "python-backend" \
  --payload-bytes 2 \
  >"$OUT_DIR/backend.log" 2>&1 &
echo $! > "$OUT_DIR/backend.pid"

"$ARC_BIN" --config "$OUT_DIR/arc.json" >"$OUT_DIR/arc.out.log" 2>"$OUT_DIR/arc.err.log" &
echo $! > "$OUT_DIR/arc.pid"

"$NGINX_BIN" -p "$OUT_DIR/nginx_runtime" -c "$OUT_DIR/nginx.conf" >"$OUT_DIR/nginx.start.log" 2>&1

wait_ready() {
  local url="$1"
  for _ in $(seq 1 80); do
    if "$CURL_BIN" -fsS --noproxy "*" "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done
  echo "service not ready: $url"
  exit 1
}

wait_ready "http://127.0.0.1:${BACKEND_PORT}/x"
wait_ready "http://127.0.0.1:${ARC_PORT}/x"
wait_ready "http://127.0.0.1:${NGINX_PORT}/x"

{
  echo "run_id=$RUN_ID"
  echo "script=benchmark/scripts/run_h1_wrk_vs_nginx.sh"
  echo "timestamp_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "git_commit=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)"
  echo "uname=$(uname -a)"
  echo "wrk_version=$("$WRK_BIN" --version 2>&1 | head -n1)"
  echo "nginx_version=$("$NGINX_BIN" -v 2>&1)"
  echo "arc_bin=$ARC_BIN"
  echo "params.RUNS=$RUNS"
  echo "params.THREADS=$THREADS"
  echo "params.CONNECTIONS=$CONNECTIONS"
  echo "params.DURATION=$DURATION"
  echo "params.WARMUP=$WARMUP"
  echo "params.REQUIRE_ZERO_NON2XX=$REQUIRE_ZERO_NON2XX"
  echo "ports.arc=$ARC_PORT"
  echo "ports.nginx=$NGINX_PORT"
  echo "ports.backend=$BACKEND_PORT"
} > "$OUT_DIR/env.txt"

run_wrk_case() {
  local name="$1"
  local url="$2"
  "$WRK_BIN" -t"$THREADS" -c"$CONNECTIONS" -d"$WARMUP" --latency "$url" >"$OUT_DIR/${name}_warmup.txt" 2>&1
  for i in $(seq 1 "$RUNS"); do
    "$WRK_BIN" -t"$THREADS" -c"$CONNECTIONS" -d"$DURATION" --latency "$url" >"$OUT_DIR/${name}_run${i}.txt" 2>&1
  done
}

run_wrk_case "backend" "http://127.0.0.1:${BACKEND_PORT}/x"
run_wrk_case "arc" "http://127.0.0.1:${ARC_PORT}/x"
run_wrk_case "nginx" "http://127.0.0.1:${NGINX_PORT}/x"

"$PYTHON_BIN" "$ROOT/benchmark/scripts/parse_wrk.py" \
  --case "backend:$OUT_DIR/backend_run*.txt" \
  --case "arc:$OUT_DIR/arc_run*.txt" \
  --case "nginx:$OUT_DIR/nginx_run*.txt" \
  --output "$OUT_DIR/summary.json" \
  --markdown "$OUT_DIR/summary.md" \
  > "$OUT_DIR/summary.stdout.json"

if [[ "$REQUIRE_ZERO_NON2XX" == "1" ]]; then
  "$PYTHON_BIN" - "$OUT_DIR/summary.json" <<'PY'
import json
import sys
from pathlib import Path

summary = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
bad = []
for name in ("arc", "nginx"):
    case = summary.get("cases", {}).get(name, {})
    non_2xx = int(case.get("non_2xx_3xx", {}).get("sum", 0))
    if non_2xx > 0:
        bad.append((name, non_2xx))
if bad:
    for name, value in bad:
        print(f"non-2xx/3xx detected in {name}: {value}", file=sys.stderr)
    sys.exit(2)
PY
fi

echo "benchmark done: $OUT_DIR"
