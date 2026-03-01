#!/usr/bin/env bash
set -euo pipefail
ROOT=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
WORK=$ROOT/tmp/wsl_arc_gateway_h1_status_c512_now
mkdir -p "$WORK/backend_runtime/logs" "$WORK/nginx_runtime/logs"

cat > "$WORK/backend_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_status_c512_now/backend.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_status_c512_now/backend_error.log warn;
  server {
    listen 127.0.0.1:19000;
    location / { return 200 "ok\n"; }
  }
}
NGINX

cat > "$WORK/proxy_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_status_c512_now/proxy.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_status_c512_now/proxy_error.log warn;
  upstream app_backend {
    server 127.0.0.1:19000;
    keepalive 256;
  }
  server {
    listen 127.0.0.1:18081;
    location / {
      proxy_http_version 1.1;
      proxy_set_header Connection "";
      proxy_set_header Host $host;
      proxy_pass http://app_backend;
    }
  }
}
NGINX

cat > "$WORK/arc.json" <<'JSON'
{
  "listen": "127.0.0.1:18080",
  "admin_listen": "127.0.0.1:19991",
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
  ]
}
JSON

cleanup(){
  set +e
  if [[ -f "$WORK/arc.pid" ]]; then
    kill "$(cat "$WORK/arc.pid")" >/dev/null 2>&1 || true
  fi
  nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
  nginx -p "$WORK/nginx_runtime" -c "$WORK/proxy_nginx.conf" -s stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

pkill -f "arc-gateway --config $WORK/arc.json" >/dev/null 2>&1 || true
nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
nginx -p "$WORK/nginx_runtime" -c "$WORK/proxy_nginx.conf" -s stop >/dev/null 2>&1 || true

nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf"
"$ROOT/target/release/arc-gateway" --config "$WORK/arc.json" > "$WORK/arc.out.log" 2> "$WORK/arc.err.log" &
echo $! > "$WORK/arc.pid"
nginx -p "$WORK/nginx_runtime" -c "$WORK/proxy_nginx.conf"

sleep 2
curl -fsS http://127.0.0.1:19000/x >/dev/null
curl -fsS http://127.0.0.1:18080/x >/dev/null
curl -fsS http://127.0.0.1:18081/x >/dev/null

wrk -t8 -c512 -d15s --latency -s "$ROOT/tmp/wrk_status.lua" http://127.0.0.1:18080/x | tee "$WORK/arc_status.txt"
wrk -t8 -c512 -d15s --latency -s "$ROOT/tmp/wrk_status.lua" http://127.0.0.1:18081/x | tee "$WORK/nginx_status.txt"
