#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/wsl_sweep_fast"
ARC_WORKERS="${ARC_WORKERS:-4}"
PINGORA_MIN_WORKERS="${PINGORA_MIN_WORKERS:-4}"
WRK_DURATION="${WRK_DURATION:-8s}"
WRK_THREADS="${WRK_THREADS:-8}"
SWEEP_CONNECTIONS="${SWEEP_CONNECTIONS:-32 64 128 256 384 512 768 1024}"

mkdir -p "$WORK/backend_runtime/logs" "$WORK/proxy_runtime/logs"

cat > "$WORK/arc.wsl.yaml" <<YAML
node:
  id: "arc-wsl-sweep-fast"
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

cat > "$WORK/backend_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_sweep_fast/backend.pid;
events {
    worker_connections 8192;
    use epoll;
}
http {
    access_log off;
    error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_sweep_fast/backend_nginx_error.log warn;
    server {
        listen 127.0.0.1:3000;
        location / {
            return 200 "ok\n";
        }
    }
}
NGINX

cat > "$WORK/proxy_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_sweep_fast/proxy.pid;
events {
    worker_connections 8192;
    use epoll;
}
http {
    access_log off;
    error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_sweep_fast/proxy_nginx_error.log warn;
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
  nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
  nginx -p "$WORK/proxy_runtime" -c "$WORK/proxy_nginx.conf" -s stop >/dev/null 2>&1 || true
}

trap cleanup EXIT

pkill -f "arc-daemon --config $WORK/arc.wsl.yaml" >/dev/null 2>&1 || true
pkill -f "pingora-min" >/dev/null 2>&1 || true
nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
nginx -p "$WORK/proxy_runtime" -c "$WORK/proxy_nginx.conf" -s stop >/dev/null 2>&1 || true

nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf"
"$ROOT/target-wsl/release/arc-daemon" --config "$WORK/arc.wsl.yaml" --log warn > "$WORK/arc.out.log" 2>&1 &
echo $! > "$WORK/arc.pid"

PINGORA_MIN_WORKERS="$PINGORA_MIN_WORKERS" PINGORA_MIN_BIND="127.0.0.1:8082" PINGORA_MIN_UPSTREAM="127.0.0.1:3000" \
  "$ROOT/target-wsl/release/pingora-min" > "$WORK/pingora_min.out.log" 2>&1 &
echo $! > "$WORK/pingora_min.pid"

nginx -p "$WORK/proxy_runtime" -c "$WORK/proxy_nginx.conf"

sleep 2

curl -fsS "http://127.0.0.1:3000/x" >/dev/null
curl -fsS "http://127.0.0.1:8080/x" >/dev/null
curl -fsS "http://127.0.0.1:8081/x" >/dev/null
curl -fsS "http://127.0.0.1:8082/x" >/dev/null

run_wrk() {
  local name="$1"
  local url="$2"
  local c="$3"
  local out="$WORK/${name}_c${c}.txt"
  echo "[sweep] ${name} c=${c}"
  wrk -t"$WRK_THREADS" -c"$c" -d"$WRK_DURATION" --latency "$url" | tee "$out"
}

for c in $SWEEP_CONNECTIONS; do
  run_wrk "backend" "http://127.0.0.1:3000/x" "$c"
  run_wrk "arc" "http://127.0.0.1:8080/x" "$c"
  run_wrk "nginx" "http://127.0.0.1:8081/x" "$c"
  run_wrk "pingora_min" "http://127.0.0.1:8082/x" "$c"
done

python3 - <<'PY'
import csv
import json
import re
from pathlib import Path

work = Path('/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_sweep_fast')

pat_rps = re.compile(r'Requests/sec:\s+([0-9.]+)')
pat_lat = re.compile(r'Latency\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)')
pat_p99 = re.compile(r'^\s*99%\s+([0-9.]+)([a-zA-Z]+)\s*$', re.M)
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

rows = []
for path in sorted(work.glob('*_c*.txt')):
    m = re.match(r'(?P<name>.+)_c(?P<c>\d+)\.txt', path.name)
    if not m:
        continue
    name = m.group('name')
    c = int(m.group('c'))
    text = path.read_text(encoding='utf-8', errors='ignore')
    rps_m = pat_rps.search(text)
    lat_m = pat_lat.search(text)
    p99_m = pat_p99.search(text)
    err_m = pat_err.search(text)

    row = {
        'name': name,
        'connections': c,
        'rps': float(rps_m.group(1)) if rps_m else 0.0,
        'lat_avg_ms': to_ms(float(lat_m.group(1)), lat_m.group(2)) if lat_m else 0.0,
        'lat_stdev_ms': to_ms(float(lat_m.group(3)), lat_m.group(4)) if lat_m else 0.0,
        'lat_max_ms': to_ms(float(lat_m.group(5)), lat_m.group(6)) if lat_m else 0.0,
        'p99_ms': to_ms(float(p99_m.group(1)), p99_m.group(2)) if p99_m else 0.0,
        'timeout_errors': int(err_m.group(4)) if err_m else 0,
    }
    rows.append(row)

rows.sort(key=lambda x: (x['connections'], x['name']))

by_c = {}
for r in rows:
    by_c.setdefault(r['connections'], {})[r['name']] = r

analysis = []
for c in sorted(by_c):
    b = by_c[c].get('backend')
    a = by_c[c].get('arc')
    n = by_c[c].get('nginx')
    p = by_c[c].get('pingora_min')
    analysis.append({
        'connections': c,
        'arc_vs_nginx_rps_ratio': (a['rps'] / n['rps']) if a and n and n['rps'] else 0.0,
        'arc_added_avg_ms': (a['lat_avg_ms'] - b['lat_avg_ms']) if a and b else 0.0,
        'nginx_added_avg_ms': (n['lat_avg_ms'] - b['lat_avg_ms']) if n and b else 0.0,
        'pingora_min_added_avg_ms': (p['lat_avg_ms'] - b['lat_avg_ms']) if p and b else 0.0,
        'arc_timeout_errors': a['timeout_errors'] if a else 0,
        'nginx_timeout_errors': n['timeout_errors'] if n else 0,
        'pingora_min_timeout_errors': p['timeout_errors'] if p else 0,
    })

summary = {
    'rows': rows,
    'analysis': analysis,
}
(work / 'summary.json').write_text(json.dumps(summary, indent=2, ensure_ascii=False), encoding='utf-8')

with (work / 'summary.csv').open('w', newline='', encoding='utf-8') as f:
    writer = csv.DictWriter(f, fieldnames=['name', 'connections', 'rps', 'lat_avg_ms', 'lat_stdev_ms', 'lat_max_ms', 'p99_ms', 'timeout_errors'])
    writer.writeheader()
    writer.writerows(rows)

print(json.dumps({'analysis': analysis}, ensure_ascii=False))
PY
