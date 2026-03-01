#!/usr/bin/env bash
set -euo pipefail

ROOT=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
WORK="$ROOT/tmp/wsl_ultra_retry_probe"
mkdir -p "$WORK/backend_runtime/logs"

cat > "$WORK/backend_nginx.conf" <<'NG'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_ultra_retry_probe/backend.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_ultra_retry_probe/backend.err warn;
  server {
    listen 127.0.0.1:3000;
    location / { return 200 "ok\n"; }
  }
}
NG

cleanup() {
  set +e
  if [[ -f "$WORK/ultra.pid" ]]; then
    kill "$(cat "$WORK/ultra.pid")" >/dev/null 2>&1 || true
  fi
  nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

pkill -f "target-ultra-wsl/release/ultra_gateway" >/dev/null 2>&1 || true
nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf"

"$ROOT/target-ultra-wsl/release/ultra_gateway" \
  --listen 127.0.0.1:8083 \
  --upstream 127.0.0.1:3000 \
  --workers 2 \
  --ring-entries 4096 \
  --accept-depth 256 \
  --conns 16384 \
  --buf-size 8192 \
  --buf-count 1024 \
  --backlog 4096 \
  > "$WORK/ultra.out.log" 2> "$WORK/ultra.err.log" &
echo $! > "$WORK/ultra.pid"
sleep 1
curl -fsS http://127.0.0.1:8083/x >/dev/null

wrk -t8 -c512 -d8s --latency http://127.0.0.1:8083/x | tee "$WORK/wrk.txt" >/dev/null
sleep 1

echo "--- wrk"
grep -m1 "Requests/sec:" "$WORK/wrk.txt"
grep -m1 "Socket errors:" "$WORK/wrk.txt" || true

echo "--- retry counts"
echo "connect_retry=$(grep -c 'connect retry' "$WORK/ultra.err.log" || true)"
echo "read_retry=$(grep -c 'read retry' "$WORK/ultra.err.log" || true)"
echo "write_retry=$(grep -c 'write retry' "$WORK/ultra.err.log" || true)"

echo "--- ring warnings"
grep -m5 "ring warning" "$WORK/ultra.err.log" || true

echo "--- sample errs"
grep -m20 -E "read failed|write failed|connect failed" "$WORK/ultra.err.log" || true