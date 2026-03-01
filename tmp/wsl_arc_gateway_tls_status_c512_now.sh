#!/usr/bin/env bash
set -euo pipefail
ROOT=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
WORK=$ROOT/tmp/wsl_arc_gateway_tls_status_c512_now
mkdir -p "$WORK/backend_runtime/logs"

openssl req -x509 -newkey rsa:2048 -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -days 1 -nodes -subj "/CN=localhost" >/dev/null 2>&1
CERT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_tls_status_c512_now/cert.pem"
KEY="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_tls_status_c512_now/key.pem"

cat > "$WORK/backend_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_tls_status_c512_now/backend.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_tls_status_c512_now/backend_error.log warn;
  server {
    listen 127.0.0.1:19000;
    location / { return 200 "ok\n"; }
  }
}
NGINX

cat > "$WORK/arc_tls.json" <<JSON
{
  "listen": "127.0.0.1:18443",
  "admin_listen": "127.0.0.1:19993",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {
    "entries": 4096,
    "accept_multishot": true,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
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
      "addr": "127.0.0.1:19000",
      "keepalive": 1024,
      "idle_ttl_ms": 30000
    }
  ],
  "plugins": [],
  "routes": [
    { "path": "/x", "upstream": "u", "plugins": [], "rate_limit": null }
  ],
  "downstream_tls": {
    "enable_h2": false,
    "certificates": [
      { "sni": "localhost", "cert_pem": "$CERT", "key_pem": "$KEY" }
    ],
    "sni_routes": []
  }
}
JSON

cleanup(){
  set +e
  if [[ -f "$WORK/arc.pid" ]]; then
    kill "$(cat "$WORK/arc.pid")" >/dev/null 2>&1 || true
  fi
  nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

pkill -f "arc-gateway --config $WORK/arc_tls.json" >/dev/null 2>&1 || true
nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true

nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf"
"$ROOT/target/release/arc-gateway" --config "$WORK/arc_tls.json" > "$WORK/arc.out.log" 2> "$WORK/arc.err.log" &
echo $! > "$WORK/arc.pid"

sleep 2
curl -sk --http1.1 https://localhost:18443/x >/dev/null
wrk -t8 -c512 -d15s --latency -s "$ROOT/tmp/wrk_status.lua" https://127.0.0.1:18443/x | tee "$WORK/arc_tls_status.txt"
