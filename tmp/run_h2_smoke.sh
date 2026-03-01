#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

killall arc-gateway >/dev/null 2>&1 || true
killall python3 >/dev/null 2>&1 || true

python3 -m http.server 19096 --bind 127.0.0.1 >/tmp/arc_h2_backend.log 2>&1 &
BPID=$!

./target/release/arc-gateway --config tmp/tls_state_check/arc.json >/tmp/arc_h2_gateway.log 2>&1 &
APID=$!

cleanup() {
  kill "$APID" "$BPID" >/dev/null 2>&1 || true
  wait "$APID" "$BPID" 2>/dev/null || true
}
trap cleanup EXIT

sleep 1.2

curl -vk --http2 --connect-timeout 3 --max-time 8 https://127.0.0.1:18448/ 2>&1 | sed -n '1,80p'