#!/usr/bin/env bash
set -euo pipefail
ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/wsl_arc_gateway_h1_vs_nginx"
WRK_DURATION="${WRK_DURATION:-10s}"
WRK_CONNECTIONS="${WRK_CONNECTIONS:-256}"
WRK_THREADS="${WRK_THREADS:-8}"
mkdir -p "$WORK/backend_runtime/logs" "$WORK/nginx_runtime/logs"

cat > "$WORK/backend_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_vs_nginx/backend.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_vs_nginx/backend_error.log warn;
  server {
    listen 127.0.0.1:19000;
    location / { return 200 "ok\n"; }
  }
}
NGINX

cat > "$WORK/proxy_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_vs_nginx/proxy.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_vs_nginx/proxy_error.log warn;
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

cleanup() {
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
curl -fsS "http://127.0.0.1:19000/x" >/dev/null
curl -fsS "http://127.0.0.1:18080/x" >/dev/null
curl -fsS "http://127.0.0.1:18081/x" >/dev/null

run_wrk() {
  local name="$1"
  local url="$2"
  for i in 1 2 3; do
    wrk -t"$WRK_THREADS" -c"$WRK_CONNECTIONS" -d"$WRK_DURATION" --latency "$url" | tee "$WORK/${name}_run${i}.txt"
  done
}

run_wrk backend "http://127.0.0.1:19000/x"
run_wrk arc "http://127.0.0.1:18080/x"
run_wrk nginx "http://127.0.0.1:18081/x"

python3 - <<'PY'
import json, re
from pathlib import Path
work = Path('/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_gateway_h1_vs_nginx')
pat_rps = re.compile(r"Requests/sec:\s+([0-9.]+)")
pat_lat = re.compile(r"Latency\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)")
pat_err = re.compile(r"Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)")
def to_ms(v, u):
    u = u.lower()
    if u == 'us': return v / 1000.0
    if u == 'ms': return v
    if u == 's': return v * 1000.0
    return v

def parse(path):
    t = path.read_text(encoding='utf-8', errors='ignore')
    mr = pat_rps.search(t)
    ml = pat_lat.search(t)
    me = pat_err.search(t)
    out = {'rps': float(mr.group(1)) if mr else 0.0, 'lat_avg_ms': 0.0, 'lat_stdev_ms': 0.0, 'lat_max_ms': 0.0, 'socket_errors': 0}
    if ml:
      out['lat_avg_ms'] = to_ms(float(ml.group(1)), ml.group(2))
      out['lat_stdev_ms'] = to_ms(float(ml.group(3)), ml.group(4))
      out['lat_max_ms'] = to_ms(float(ml.group(5)), ml.group(6))
    if me:
      out['socket_errors'] = sum(int(x) for x in me.groups())
    return out

def agg(name):
    rows = [parse(work / f"{name}_run{i}.txt") for i in (1, 2, 3)]
    n = len(rows)
    return {
      'runs': rows,
      'avg': {
        'rps': sum(r['rps'] for r in rows) / n,
        'lat_avg_ms': sum(r['lat_avg_ms'] for r in rows) / n,
        'lat_stdev_ms': sum(r['lat_stdev_ms'] for r in rows) / n,
        'lat_max_ms': sum(r['lat_max_ms'] for r in rows) / n,
        'socket_errors': sum(r['socket_errors'] for r in rows),
      }
    }

s = {'backend': agg('backend'), 'arc': agg('arc'), 'nginx': agg('nginx')}
b = s['backend']['avg']['rps']
a = s['arc']['avg']['rps']
n = s['nginx']['avg']['rps']
lat_b = s['backend']['avg']['lat_avg_ms']
s['compare'] = {
  'arc_vs_nginx_rps_ratio': a / n if n else 0.0,
  'arc_vs_nginx_rps_gap_pct': ((n - a) / n * 100.0) if n else 0.0,
  'arc_vs_backend_rps_ratio': a / b if b else 0.0,
  'nginx_vs_backend_rps_ratio': n / b if b else 0.0,
}
s['latency_added_ms'] = {
  'arc': s['arc']['avg']['lat_avg_ms'] - lat_b,
  'nginx': s['nginx']['avg']['lat_avg_ms'] - lat_b,
}
(work / 'summary.json').write_text(json.dumps(s, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps(s, ensure_ascii=False))
PY
