#!/usr/bin/env bash
set -euo pipefail

ROOT=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
WORK="$ROOT/tmp/wsl_ultra_leak_check"
mkdir -p "$WORK/backend_runtime/logs"

cat > "$WORK/backend_nginx.conf" <<'NG'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_ultra_leak_check/backend.pid;
events { worker_connections 8192; use epoll; }
http {
  access_log off;
  error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_ultra_leak_check/backend.err warn;
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

start_ultra() {
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
}

parse_run() {
  local run_file="$1"
  local rps errs
  rps=$(grep -m1 "Requests/sec:" "$run_file" | awk '{print $2}')
  errs=$(grep -m1 "Socket errors:" "$run_file" || true)
  echo "$rps|$errs"
}

echo "=== scenario A: one ultra, 3 sequential runs ==="
start_ultra
UPID=$(cat "$WORK/ultra.pid")
for i in 1 2 3; do
  wrk -t8 -c512 -d8s --latency http://127.0.0.1:8083/x | tee "$WORK/a_run${i}.txt" >/dev/null
  sleep 1
  fdcount=$(ls "/proc/$UPID/fd" | wc -l)
  established=$(ss -tan state established \( sport = :8083 \) | tail -n +2 | wc -l)
  parsed=$(parse_run "$WORK/a_run${i}.txt")
  rps=${parsed%%|*}
  errs=${parsed#*|}
  echo "A run${i}: rps=${rps} fd=${fdcount} established=${established} ${errs}"
done
kill "$UPID" >/dev/null 2>&1 || true
sleep 1

echo "=== scenario B: restart ultra each run ==="
for i in 1 2 3; do
  start_ultra
  UPID=$(cat "$WORK/ultra.pid")
  wrk -t8 -c512 -d8s --latency http://127.0.0.1:8083/x | tee "$WORK/b_run${i}.txt" >/dev/null
  sleep 1
  fdcount=$(ls "/proc/$UPID/fd" | wc -l)
  established=$(ss -tan state established \( sport = :8083 \) | tail -n +2 | wc -l)
  parsed=$(parse_run "$WORK/b_run${i}.txt")
  rps=${parsed%%|*}
  errs=${parsed#*|}
  echo "B run${i}: rps=${rps} fd=${fdcount} established=${established} ${errs}"
  kill "$UPID" >/dev/null 2>&1 || true
  sleep 1
done