#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

python3 /tmp/arc_limit_backend.py >/tmp/arc_limit_backend.log 2>&1 &
BACK_PID=$!

./target/debug/arc-gateway --config /tmp/arc_limit.json >/tmp/arc_limit_gateway.log 2>&1 &
GW_PID=$!

cleanup(){
  set +e
  kill "$GW_PID" >/dev/null 2>&1 || true
  kill "$BACK_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for i in $(seq 1 20); do
  if curl -sS http://127.0.0.1:18180/ >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

codes=""
for i in $(seq 1 5); do
  c=$(curl -sS -o /dev/null -w '%{http_code}' http://127.0.0.1:18180/)
  codes="$codes $c"
  sleep 1.2
done

echo "SLOW_CODES=$codes"
