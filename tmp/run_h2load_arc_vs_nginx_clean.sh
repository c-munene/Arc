#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

ARC_PORT="${ARC_PORT:-19448}"
NGX_PORT="${NGX_PORT:-19449}"
BACKEND_PORT="${BACKEND_PORT:-19096}"
ADMIN_PORT="${ADMIN_PORT:-19997}"

H2_REQS="${H2_REQS:-20000}"
H2_CLIENTS="${H2_CLIENTS:-64}"
H2_STREAMS="${H2_STREAMS:-20}"
H2_THREADS="${H2_THREADS:-2}"
H2_CASE_TIMEOUT="${H2_CASE_TIMEOUT:-240}"

H2LOAD_BIN="/tmp/nghttp2-client-extract/usr/bin/h2load"
H2LIB_PATH="/tmp/nghttp2-libs/usr/lib/x86_64-linux-gnu:/tmp/nghttp2-libs/lib/x86_64-linux-gnu"

if [[ ! -x "$H2LOAD_BIN" ]]; then
  echo "missing h2load at $H2LOAD_BIN"
  exit 1
fi

cleanup_ports() {
  for port in "$ARC_PORT" "$NGX_PORT" "$BACKEND_PORT" "$ADMIN_PORT"; do
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
    if [[ -n "${pids}" ]]; then
      # shellcheck disable=SC2086
      kill -9 ${pids} >/dev/null 2>&1 || true
    fi
  done
}

cleanup_ports
mkdir -p /tmp/arc_h2_clean_backend
python3 - <<'PY'
from pathlib import Path
p = Path("/tmp/arc_h2_clean_backend/index.html")
p.write_text("x" * 4096)
PY

cat > /tmp/arc_h2_clean_backend.conf <<'NGX'
pid /tmp/arc_h2_clean_backend.pid;
events { worker_connections 4096; }
http {
    access_log off;
    sendfile on;
    keepalive_timeout 65;
    server {
        listen 19096;
        root /tmp/arc_h2_clean_backend;
        location / {
            try_files $uri /index.html;
        }
    }
}
NGX

cat > /tmp/arc_h2_clean_proxy.conf <<'NGX'
pid /tmp/arc_h2_clean_proxy.pid;
events { worker_connections 4096; }
http {
    access_log off;
    sendfile on;
    server {
        listen 19449 ssl http2;
        ssl_certificate /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem;
        ssl_certificate_key /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem;
        ssl_protocols TLSv1.2 TLSv1.3;
        location / {
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_pass http://127.0.0.1:19096;
        }
    }
}
NGX

cat > /tmp/arc_h2_clean_arc.json <<'ARC'
{
  "listen": "127.0.0.1:19448",
  "admin_listen": "127.0.0.1:19997",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "accept_prepost": 32, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "listen_backlog": 4096,
  "buffers": {"buf_size": 8192, "buf_count": 256},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 120000, "up_conn": 3000, "up_handshake": 1000, "up_write": 120000, "up_read": 120000, "cli_write": 120000},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19096", "keepalive": 256, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "downstream_tls": {"enable_h2": true, "certificates": [{"sni": "localhost", "cert_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem", "key_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem"}], "sni_routes": []}
}
ARC

nginx -c /tmp/arc_h2_clean_backend.conf -p /tmp/ >/tmp/arc_h2_clean_backend.log 2>&1
./target/release/arc-gateway --config /tmp/arc_h2_clean_arc.json >/tmp/arc_h2_clean_arc.log 2>&1 &
APID=$!
nginx -c /tmp/arc_h2_clean_proxy.conf -p /tmp/ >/tmp/arc_h2_clean_proxy.log 2>&1

cleanup() {
  nginx -c /tmp/arc_h2_clean_proxy.conf -p /tmp/ -s stop >/dev/null 2>&1 || true
  nginx -c /tmp/arc_h2_clean_backend.conf -p /tmp/ -s stop >/dev/null 2>&1 || true
  kill "$APID" >/dev/null 2>&1 || true
  wait "$APID" 2>/dev/null || true
  cleanup_ports
}
trap cleanup EXIT

sleep 1

run_case() {
  local name="$1"
  local uri="$2"
  local out="$3"
  if ! timeout "${H2_CASE_TIMEOUT}s" env LD_LIBRARY_PATH="$H2LIB_PATH" "$H2LOAD_BIN" \
    -n "${H2_REQS}" \
    -c "${H2_CLIENTS}" \
    -m "${H2_STREAMS}" \
    -t "${H2_THREADS}" \
    --sni localhost \
    "$uri" >"$out" 2>&1; then
    echo "h2load ${name} timed out or failed"
    cat "$out" || true
    exit 1
  fi
  echo "=== ${name} ==="
  cat "$out"
}

run_case ARC "https://127.0.0.1:${ARC_PORT}/" "/tmp/arc_h2_clean_arc.txt"
run_case NGINX "https://127.0.0.1:${NGX_PORT}/" "/tmp/arc_h2_clean_nginx.txt"

arc_rps="$(grep -Eo 'finished in [^,]+, [0-9.]+ req/s' /tmp/arc_h2_clean_arc.txt | tail -n1 | awk '{print $(NF-1)}')"
ngx_rps="$(grep -Eo 'finished in [^,]+, [0-9.]+ req/s' /tmp/arc_h2_clean_nginx.txt | tail -n1 | awk '{print $(NF-1)}')"
arc_req_line="$(grep -E '^requests:' /tmp/arc_h2_clean_arc.txt | tail -n1)"
ngx_req_line="$(grep -E '^requests:' /tmp/arc_h2_clean_nginx.txt | tail -n1)"
arc_fail="$(echo "$arc_req_line" | sed -E 's/.* ([0-9]+) failed.*/\1/')"
ngx_fail="$(echo "$ngx_req_line" | sed -E 's/.* ([0-9]+) failed.*/\1/')"
arc_err="$(echo "$arc_req_line" | sed -E 's/.* ([0-9]+) errored.*/\1/')"
ngx_err="$(echo "$ngx_req_line" | sed -E 's/.* ([0-9]+) errored.*/\1/')"
arc_ok="$(echo "$arc_req_line" | sed -E 's/.* done, ([0-9]+) succeeded.*/\1/')"
ngx_ok="$(echo "$ngx_req_line" | sed -E 's/.* done, ([0-9]+) succeeded.*/\1/')"

echo "=== SUMMARY ==="
echo "ARC_RPS=${arc_rps}"
echo "NGINX_RPS=${ngx_rps}"
echo "ARC_SUCCEEDED=${arc_ok}"
echo "NGINX_SUCCEEDED=${ngx_ok}"
echo "ARC_FAILED=${arc_fail}"
echo "NGINX_FAILED=${ngx_fail}"
echo "ARC_ERRORED=${arc_err}"
echo "NGINX_ERRORED=${ngx_err}"
if [[ -n "${arc_rps}" && -n "${ngx_rps}" ]]; then
  python3 - "$arc_rps" "$ngx_rps" <<'PY'
import sys
arc=float(sys.argv[1]); ngx=float(sys.argv[2])
print(f"RPS_RATIO_ARC_OVER_NGINX={arc/ngx:.4f}")
PY
fi
