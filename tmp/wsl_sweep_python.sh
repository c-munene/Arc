#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/wsl_sweep_python"
ARC_WORKERS="${ARC_WORKERS:-4}"
PINGORA_MIN_WORKERS="${PINGORA_MIN_WORKERS:-4}"
WRK_DURATION="${WRK_DURATION:-8s}"
WRK_THREADS="${WRK_THREADS:-8}"
SWEEP_CONNECTIONS="${SWEEP_CONNECTIONS:-64 128 192 256 320}"

mkdir -p "$WORK/backend_root" "$WORK/nginx_runtime/logs"
echo "ok" > "$WORK/backend_root/x"

cat > "$WORK/arc.wsl.yaml" <<YAML
node:
  id: "arc-wsl-sweep-python"
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
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_sweep_python/nginx.pid;

events {
    worker_connections 8192;
    use epoll;
}

http {
    access_log off;
    error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_sweep_python/nginx_error.log warn;

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
  if [[ -f "$WORK/backend.pid" ]]; then
    kill "$(cat "$WORK/backend.pid")" >/dev/null 2>&1 || true
  fi
  nginx -p "$WORK/nginx_runtime" -c "$WORK/nginx.conf" -s stop >/dev/null 2>&1 || true
}

trap cleanup EXIT

pkill -f "python3 -m http.server 3000" >/dev/null 2>&1 || true
pkill -f "arc-daemon --config $WORK/arc.wsl.yaml" >/dev/null 2>&1 || true
pkill -f "pingora-min" >/dev/null 2>&1 || true
nginx -p "$WORK/nginx_runtime" -c "$WORK/nginx.conf" -s stop >/dev/null 2>&1 || true

python3 -m http.server 3000 --bind 127.0.0.1 --directory "$WORK/backend_root" > "$WORK/backend.out.log" 2>&1 &
echo $! > "$WORK/backend.pid"

"$ROOT/target-wsl/release/arc-daemon" --config "$WORK/arc.wsl.yaml" --log warn > "$WORK/arc.out.log" 2>&1 &
echo $! > "$WORK/arc.pid"

PINGORA_MIN_WORKERS="$PINGORA_MIN_WORKERS" PINGORA_MIN_BIND="127.0.0.1:8082" PINGORA_MIN_UPSTREAM="127.0.0.1:3000" \
  "$ROOT/target-wsl/release/pingora-min" > "$WORK/pingora_min.out.log" 2>&1 &
echo $! > "$WORK/pingora_min.pid"

nginx -p "$WORK/nginx_runtime" -c "$WORK/nginx.conf"

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
