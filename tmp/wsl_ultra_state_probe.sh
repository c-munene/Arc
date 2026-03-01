#!/usr/bin/env bash
set -euo pipefail
ROOT=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
WORK=$ROOT/tmp/wsl_ultra_state_probe
mkdir -p "$WORK"

pkill -f "python3 -m http.server 3000" >/dev/null 2>&1 || true
pkill -f "target-ultra-wsl/release/ultra_gateway" >/dev/null 2>&1 || true

python3 -m http.server 3000 --bind 127.0.0.1 --directory "$WORK" > "$WORK/backend.log" 2>&1 &
BPID=$!

"$ROOT/target-ultra-wsl/release/ultra_gateway" \
  --http1 \
  --http1-upstream-pool 1024 \
  --http1-io-timeout-ms 30000 \
  --listen 127.0.0.1:8083 \
  --upstream 127.0.0.1:3000 \
  --workers 2 \
  --ring-entries 4096 \
  --accept-depth 256 \
  --conns 16384 \
  --buf-size 8192 \
  --buf-count 4096 \
  --backlog 4096 \
  > "$WORK/ultra.out.log" 2> "$WORK/ultra.err.log" &
UPID=$!

cleanup(){
  kill "$UPID" >/dev/null 2>&1 || true
  kill "$BPID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 2
for i in 1 2 3; do
  echo "=== run$i start ==="
  wrk -t8 -c256 -d10s --latency http://127.0.0.1:8083/x | tee "$WORK/run${i}.txt" >/dev/null
  echo "--- run$i ss sport=8083 ---"
  ss -tan '( sport = :8083 )' | awk 'NR>1{a[$1]++} END{for(k in a) print k,a[k]}' | sort
  echo "--- run$i ss dport=8083 ---"
  ss -tan '( dport = :8083 )' | awk 'NR>1{a[$1]++} END{for(k in a) print k,a[k]}' | sort
  echo "--- run$i fd count ---"
  ls /proc/$UPID/fd | wc -l
  echo "=== run$i end ==="
  sleep 1
done
