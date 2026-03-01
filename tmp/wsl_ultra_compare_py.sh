#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/wsl_ultra_compare_py"
ARC_WORKERS="${ARC_WORKERS:-2}"
PINGORA_MIN_WORKERS="${PINGORA_MIN_WORKERS:-2}"
ULTRA_WORKERS="${ULTRA_WORKERS:-2}"
ULTRA_BUF_COUNT="${ULTRA_BUF_COUNT:-4096}"
WRK_DURATION="${WRK_DURATION:-10s}"
WRK_CONNECTIONS="${WRK_CONNECTIONS:-256}"
WRK_THREADS="${WRK_THREADS:-8}"

mkdir -p "$WORK/backend_root" "$WORK/nginx_runtime/logs"
echo "ok" > "$WORK/backend_root/x"

cat > "$WORK/arc.wsl.yaml" <<YAML
node:
  id: "arc-wsl-ultra-compare-py"
  workers: $ARC_WORKERS
  max_connections: 0
  read_timeout: 30s
  write_timeout: 30s
  idle_timeout: 60s

listeners:
  - name: "http"
    kind: http
    bind: "127.0.0.1:8080"
    socket:
      so_reuseport: true

upstreams:
  - name: "app"
    discovery:
      type: static
      endpoints:
        - address: "127.0.0.1:3000"
          weight: 1
    lb:
      algorithm: peak_ewma
      decay: 10s
    pool:
      max_idle: 1024
      idle_ttl: 30s
      max_lifetime: 300s

routes:
  - name: "root"
    match:
      host: []
      methods: []
      path: "/{*rest}"
      headers: []
      cookies: []
      query: []
    action:
      upstream: "app"
      retry:
        max_retries: 1
        backoff: 50ms
        idempotent_only: true

plugins: {}

observability:
  metrics_enabled: false
  metrics_bind: "127.0.0.1:9090"
  access_log:
    enabled: false
    sample: 1.0
    disabled_routes: []

control_plane:
  enabled: false
  bind: "127.0.0.1:9900"
YAML

cat > "$WORK/nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_ultra_compare_py/nginx.pid;

events {
    worker_connections 8192;
    use epoll;
}

http {
    access_log off;
    error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_ultra_compare_py/nginx_error.log warn;

    upstream app_backend {
        server 127.0.0.1:3000;
        keepalive 256;
    }

    server {
        listen 127.0.0.1:8081;
        location / {
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_set_header Host $host;
            proxy_pass http://app_backend;
        }
    }
}
NGINX

cleanup() {
  set +e
  if [[ -f "$WORK/arc.pid" ]]; then
    kill "$(cat "$WORK/arc.pid")" >/dev/null 2>&1 || true
  fi
  if [[ -f "$WORK/pingora_min.pid" ]]; then
    kill "$(cat "$WORK/pingora_min.pid")" >/dev/null 2>&1 || true
  fi
  if [[ -f "$WORK/ultra.pid" ]]; then
    kill "$(cat "$WORK/ultra.pid")" >/dev/null 2>&1 || true
  fi
  if [[ -f "$WORK/backend.pid" ]]; then
    kill "$(cat "$WORK/backend.pid")" >/dev/null 2>&1 || true
  fi
  nginx -p "$WORK/nginx_runtime" -c "$WORK/nginx.conf" -s stop >/dev/null 2>&1 || true
}

trap cleanup EXIT

pkill -f "python3 -m http.server 3000" >/dev/null 2>&1 || true
pkill -f "arc-daemon --config $WORK/arc.wsl.yaml" >/dev/null 2>&1 || true
pkill -f "pingora-min" >/dev/null 2>&1 || true
pkill -f "target-ultra-wsl/release/ultra_gateway" >/dev/null 2>&1 || true
nginx -p "$WORK/nginx_runtime" -c "$WORK/nginx.conf" -s stop >/dev/null 2>&1 || true

python3 -m http.server 3000 --bind 127.0.0.1 --directory "$WORK/backend_root" > "$WORK/backend.out.log" 2>&1 &
echo $! > "$WORK/backend.pid"

"$ROOT/target-wsl/release/arc-daemon" --config "$WORK/arc.wsl.yaml" --log warn > "$WORK/arc.out.log" 2>&1 &
echo $! > "$WORK/arc.pid"

PINGORA_MIN_WORKERS="$PINGORA_MIN_WORKERS" PINGORA_MIN_BIND="127.0.0.1:8082" PINGORA_MIN_UPSTREAM="127.0.0.1:3000" \
  "$ROOT/target-wsl/release/pingora-min" > "$WORK/pingora_min.out.log" 2>&1 &
echo $! > "$WORK/pingora_min.pid"

"$ROOT/target-ultra-wsl/release/ultra_gateway" \
  --listen 127.0.0.1:8083 \
  --upstream 127.0.0.1:3000 \
  --workers "$ULTRA_WORKERS" \
  --ring-entries 4096 \
  --accept-depth 256 \
  --conns 16384 \
  --buf-size 8192 \
  --buf-count "$ULTRA_BUF_COUNT" \
  --backlog 4096 \
  > "$WORK/ultra.out.log" 2> "$WORK/ultra.err.log" &
echo $! > "$WORK/ultra.pid"

nginx -p "$WORK/nginx_runtime" -c "$WORK/nginx.conf"

sleep 2

curl -fsS "http://127.0.0.1:3000/x" >/dev/null
curl -fsS "http://127.0.0.1:8080/x" >/dev/null
curl -fsS "http://127.0.0.1:8081/x" >/dev/null
curl -fsS "http://127.0.0.1:8082/x" >/dev/null
curl -fsS "http://127.0.0.1:8083/x" >/dev/null

run_wrk() {
  local name="$1"
  local url="$2"
  for i in 1 2 3; do
    wrk -t"$WRK_THREADS" -c"$WRK_CONNECTIONS" -d"$WRK_DURATION" --latency "$url" | tee "$WORK/${name}_run${i}.txt"
  done
}

run_wrk "backend" "http://127.0.0.1:3000/x"
run_wrk "arc" "http://127.0.0.1:8080/x"
run_wrk "nginx" "http://127.0.0.1:8081/x"
run_wrk "pingora_min" "http://127.0.0.1:8082/x"
run_wrk "ultra" "http://127.0.0.1:8083/x"

python3 - <<'PY'
import json
import re
from pathlib import Path

work = Path('/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_ultra_compare_py')
pat_rps = re.compile(r'Requests/sec:\s+([0-9.]+)')
pat_lat = re.compile(r'Latency\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)')
pat_err = re.compile(r'Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)')

def to_ms(v, unit):
    unit = unit.lower()
    if unit == 'us':
        return v / 1000.0
    if unit == 'ms':
        return v
    if unit == 's':
        return v * 1000.0
    return v

def parse_file(path: Path):
    text = path.read_text(encoding='utf-8', errors='ignore')
    m_rps = pat_rps.search(text)
    m_lat = pat_lat.search(text)
    m_err = pat_err.search(text)
    out = {
        'rps': float(m_rps.group(1)) if m_rps else 0.0,
        'lat_avg_ms': 0.0,
        'lat_stdev_ms': 0.0,
        'lat_max_ms': 0.0,
        'socket_errors': 0,
    }
    if m_lat:
        out['lat_avg_ms'] = to_ms(float(m_lat.group(1)), m_lat.group(2))
        out['lat_stdev_ms'] = to_ms(float(m_lat.group(3)), m_lat.group(4))
        out['lat_max_ms'] = to_ms(float(m_lat.group(5)), m_lat.group(6))
    if m_err:
        out['socket_errors'] = sum(int(x) for x in m_err.groups())
    return out

def aggregate(name: str):
    rows = [parse_file(work / f"{name}_run{i}.txt") for i in range(1, 4)]
    n = len(rows)
    return {
        'runs': rows,
        'avg': {
            'rps': sum(r['rps'] for r in rows) / n,
            'lat_avg_ms': sum(r['lat_avg_ms'] for r in rows) / n,
            'lat_stdev_ms': sum(r['lat_stdev_ms'] for r in rows) / n,
            'lat_max_ms': sum(r['lat_max_ms'] for r in rows) / n,
            'socket_errors': sum(r['socket_errors'] for r in rows),
        },
    }

summary = {
    'backend': aggregate('backend'),
    'arc': aggregate('arc'),
    'nginx': aggregate('nginx'),
    'pingora_min': aggregate('pingora_min'),
    'ultra': aggregate('ultra'),
}

summary['compare'] = {
    'arc_vs_nginx_rps_ratio': summary['arc']['avg']['rps'] / summary['nginx']['avg']['rps'] if summary['nginx']['avg']['rps'] else 0.0,
    'ultra_vs_nginx_rps_ratio': summary['ultra']['avg']['rps'] / summary['nginx']['avg']['rps'] if summary['nginx']['avg']['rps'] else 0.0,
    'ultra_vs_arc_rps_ratio': summary['ultra']['avg']['rps'] / summary['arc']['avg']['rps'] if summary['arc']['avg']['rps'] else 0.0,
    'arc_vs_backend_rps_ratio': summary['arc']['avg']['rps'] / summary['backend']['avg']['rps'] if summary['backend']['avg']['rps'] else 0.0,
    'nginx_vs_backend_rps_ratio': summary['nginx']['avg']['rps'] / summary['backend']['avg']['rps'] if summary['backend']['avg']['rps'] else 0.0,
    'ultra_vs_backend_rps_ratio': summary['ultra']['avg']['rps'] / summary['backend']['avg']['rps'] if summary['backend']['avg']['rps'] else 0.0,
}

(work / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps(summary, ensure_ascii=False))
PY
