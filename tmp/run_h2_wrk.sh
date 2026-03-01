#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

killall arc-gateway >/dev/null 2>&1 || true
killall python3 >/dev/null 2>&1 || true

python3 -m http.server 19096 --bind 127.0.0.1 >/tmp/arc_h2_wrk_backend.log 2>&1 &
BPID=$!
./target/release/arc-gateway --config tmp/tls_state_check/arc.json >/tmp/arc_h2_wrk_gateway.log 2>&1 &
GPID=$!

cleanup() {
  kill "$GPID" "$BPID" >/dev/null 2>&1 || true
  wait "$GPID" "$BPID" 2>/dev/null || true
}
trap cleanup EXIT

sleep 1.0
wrk -t2 -c32 -d8s --latency https://127.0.0.1:18448/ >/tmp/arc_h2_wrk.txt 2>&1 || true
cat /tmp/arc_h2_wrk.txt
