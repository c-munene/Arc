#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RESULT_ROOT="$ROOT/benchmark/results"
RUN_ID="${RUN_ID:-$(date +%Y%m%d_%H%M%S)}"
OUT_DIR="${OUT_DIR:-$RESULT_ROOT/h2_h2load_${RUN_ID}}"

H2LOAD_BIN="${H2LOAD_BIN:-h2load}"
NGINX_BIN="${NGINX_BIN:-nginx}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
OPENSSL_BIN="${OPENSSL_BIN:-openssl}"
ARC_BIN="${ARC_BIN:-$ROOT/target/release/arc-gateway}"
CURL_BIN="${CURL_BIN:-curl}"

RUNS="${RUNS:-5}"
REQUESTS="${REQUESTS:-20000}"
CLIENTS="${CLIENTS:-64}"
STREAMS="${STREAMS:-20}"
THREADS="${THREADS:-2}"
WARMUP_REQUESTS="${WARMUP_REQUESTS:-1000}"
CASE_TIMEOUT_SEC="${CASE_TIMEOUT_SEC:-240}"

ARC_PORT="${ARC_PORT:-19448}"
NGINX_PORT="${NGINX_PORT:-19449}"
BACKEND_PORT="${BACKEND_PORT:-19096}"
ADMIN_PORT="${ADMIN_PORT:-19997}"
ARC_WORKERS="${ARC_WORKERS:-1}"

mkdir -p "$OUT_DIR"

require_bin() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    echo "missing required command: $name"
    exit 1
  fi
}

require_bin "$H2LOAD_BIN"
require_bin "$NGINX_BIN"
require_bin "$PYTHON_BIN"
require_bin "$OPENSSL_BIN"
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

CERT_PEM="$OUT_DIR/localhost.crt.pem"
KEY_PEM="$OUT_DIR/localhost.key.pem"

"$OPENSSL_BIN" req -x509 -newkey rsa:2048 -sha256 -days 1 -nodes \
  -subj "/CN=localhost" \
  -keyout "$KEY_PEM" -out "$CERT_PEM" >/dev/null 2>&1

cat > "$OUT_DIR/nginx.conf" <<EOF
worker_processes auto;
pid $OUT_DIR/nginx.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log $OUT_DIR/nginx.error.log warn;
  server {
    listen 127.0.0.1:${NGINX_PORT} ssl http2;
    ssl_certificate $CERT_PEM;
    ssl_certificate_key $KEY_PEM;
    ssl_protocols TLSv1.2 TLSv1.3;
    location / {
      proxy_http_version 1.1;
      proxy_set_header Connection "";
      proxy_set_header Host \$host;
      proxy_pass http://127.0.0.1:${BACKEND_PORT};
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
    "cli_read": 120000,
    "up_conn": 3000,
    "up_handshake": 1000,
    "up_write": 120000,
    "up_read": 120000,
    "cli_write": 120000
  },
  "require_upstream_mtls": false,
  "upstreams": [
    {
      "name": "u",
      "addr": "127.0.0.1:${BACKEND_PORT}",
      "keepalive": 256,
      "idle_ttl_ms": 30000
    }
  ],
  "plugins": [],
  "routes": [
    { "path": "/x", "upstream": "u", "plugins": [], "rate_limit": null }
  ],
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [
      {
        "sni": "localhost",
        "cert_pem": "$CERT_PEM",
        "key_pem": "$KEY_PEM"
      }
    ],
    "sni_routes": []
  }
}
EOF

"$PYTHON_BIN" "$ROOT/benchmark/backends/http_ok_backend.py" \
  --port "$BACKEND_PORT" \
  --name "python-backend" \
  --payload-bytes 4096 \
  >"$OUT_DIR/backend.log" 2>&1 &
echo $! > "$OUT_DIR/backend.pid"

"$ARC_BIN" --config "$OUT_DIR/arc.json" >"$OUT_DIR/arc.out.log" 2>"$OUT_DIR/arc.err.log" &
echo $! > "$OUT_DIR/arc.pid"

"$NGINX_BIN" -p "$OUT_DIR/nginx_runtime" -c "$OUT_DIR/nginx.conf" >"$OUT_DIR/nginx.start.log" 2>&1

wait_ready_https() {
  local url="$1"
  for _ in $(seq 1 100); do
    if "$CURL_BIN" -k -fsS --http2 --noproxy "*" "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done
  echo "service not ready: $url"
  exit 1
}

wait_ready_https "https://127.0.0.1:${ARC_PORT}/x"
wait_ready_https "https://127.0.0.1:${NGINX_PORT}/x"

{
  echo "run_id=$RUN_ID"
  echo "script=benchmark/scripts/run_h2_h2load_vs_nginx.sh"
  echo "timestamp_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "git_commit=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)"
  echo "uname=$(uname -a)"
  echo "h2load_version=$("$H2LOAD_BIN" --version 2>&1 | head -n1)"
  echo "nginx_version=$("$NGINX_BIN" -v 2>&1)"
  echo "arc_bin=$ARC_BIN"
  echo "params.RUNS=$RUNS"
  echo "params.REQUESTS=$REQUESTS"
  echo "params.CLIENTS=$CLIENTS"
  echo "params.STREAMS=$STREAMS"
  echo "params.THREADS=$THREADS"
  echo "params.WARMUP_REQUESTS=$WARMUP_REQUESTS"
  echo "params.CASE_TIMEOUT_SEC=$CASE_TIMEOUT_SEC"
  echo "ports.arc=$ARC_PORT"
  echo "ports.nginx=$NGINX_PORT"
  echo "ports.backend=$BACKEND_PORT"
} > "$OUT_DIR/env.txt"

H2LOAD_TLS_FLAGS=()
if "$H2LOAD_BIN" --help 2>&1 | grep -q -- '--insecure'; then
  H2LOAD_TLS_FLAGS+=(--insecure)
elif "$H2LOAD_BIN" --help 2>&1 | grep -q -- '--no-verify-peer'; then
  H2LOAD_TLS_FLAGS+=(--no-verify-peer)
elif "$H2LOAD_BIN" --help 2>&1 | grep -q -- '-k'; then
  H2LOAD_TLS_FLAGS+=(-k)
fi

run_h2_case() {
  local name="$1"
  local url="$2"

  timeout "${CASE_TIMEOUT_SEC}s" "$H2LOAD_BIN" \
    -n "$WARMUP_REQUESTS" \
    -c "$CLIENTS" \
    -m "$STREAMS" \
    -t "$THREADS" \
    --sni localhost \
    "${H2LOAD_TLS_FLAGS[@]}" \
    "$url" >"$OUT_DIR/${name}_warmup.txt" 2>&1

  for i in $(seq 1 "$RUNS"); do
    timeout "${CASE_TIMEOUT_SEC}s" "$H2LOAD_BIN" \
      -n "$REQUESTS" \
      -c "$CLIENTS" \
      -m "$STREAMS" \
      -t "$THREADS" \
      --sni localhost \
      "${H2LOAD_TLS_FLAGS[@]}" \
      "$url" >"$OUT_DIR/${name}_run${i}.txt" 2>&1
  done
}

run_h2_case "arc" "https://127.0.0.1:${ARC_PORT}/x"
run_h2_case "nginx" "https://127.0.0.1:${NGINX_PORT}/x"

"$PYTHON_BIN" "$ROOT/benchmark/scripts/parse_h2load.py" \
  --case "arc:$OUT_DIR/arc_run*.txt" \
  --case "nginx:$OUT_DIR/nginx_run*.txt" \
  --output "$OUT_DIR/summary.json" \
  --markdown "$OUT_DIR/summary.md" \
  > "$OUT_DIR/summary.stdout.json"

echo "benchmark done: $OUT_DIR"
