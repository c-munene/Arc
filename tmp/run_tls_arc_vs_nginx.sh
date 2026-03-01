#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

cleanup_ports() {
  for port in 19448 19449 19096 19997; do
    pids="$(
      ss -ltnp 2>/dev/null \
        | grep -E "[\\.:]${port}[[:space:]]" \
        | sed -n 's/.*pid=\([0-9]\+\).*/\1/p' \
        | sort -u \
        || true
    )"
    if [[ -n "${pids}" ]]; then
      # shellcheck disable=SC2086
      kill -9 ${pids} >/dev/null 2>&1 || true
    fi
  done
}

cleanup_ports

cat > /tmp/arc_tls_vs_nginx.conf <<'NGX'
pid /tmp/arc_tls_vs_nginx.pid;
events { worker_connections 2048; }
http {
    access_log off;
    sendfile on;
    server {
        listen 19449 ssl;
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

cat > /tmp/arc_tls_vs_nginx_arc.json <<'ARC'
{
  "listen": "127.0.0.1:19448",
  "admin_listen": "127.0.0.1:19997",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 64},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 30000, "up_conn": 3000, "up_handshake": 1000, "up_write": 30000, "up_read": 30000, "cli_write": 30000},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19096", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "downstream_tls": {"enable_h2": true, "certificates": [{"sni": "localhost", "cert_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem", "key_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem"}], "sni_routes": []}
}
ARC

python3 -m http.server 19096 --bind 127.0.0.1 >/tmp/arc_tls_vs_nginx_backend.log 2>&1 &
BPID=$!
./target/release/arc-gateway --config /tmp/arc_tls_vs_nginx_arc.json >/tmp/arc_tls_vs_nginx_arc.log 2>&1 &
APID=$!
nginx -c /tmp/arc_tls_vs_nginx.conf -p /tmp/ >/tmp/arc_tls_vs_nginx_nginx.log 2>&1

cleanup() {
  nginx -c /tmp/arc_tls_vs_nginx.conf -p /tmp/ -s stop >/dev/null 2>&1 || true
  kill "$APID" "$BPID" >/dev/null 2>&1 || true
  wait "$APID" "$BPID" 2>/dev/null || true
}
trap cleanup EXIT

sleep 1.0

run_case() {
  local name="$1"
  local threads="$2"
  local conns="$3"
  local dur="$4"

  wrk -t"${threads}" -c"${conns}" -d"${dur}" --latency https://127.0.0.1:19448/ >"/tmp/arc_tls_wrk_arc_${name}.txt" 2>&1
  wrk -t"${threads}" -c"${conns}" -d"${dur}" --latency https://127.0.0.1:19449/ >"/tmp/arc_tls_wrk_nginx_${name}.txt" 2>&1

  echo "=== CASE ${name} ARC ==="
  cat "/tmp/arc_tls_wrk_arc_${name}.txt"
  echo "=== CASE ${name} NGINX ==="
  cat "/tmp/arc_tls_wrk_nginx_${name}.txt"
}

run_case c32 2 32 8s
run_case c128 4 128 8s
