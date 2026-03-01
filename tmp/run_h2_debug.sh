#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

killall arc-gateway >/dev/null 2>&1 || true
killall python3 >/dev/null 2>&1 || true
# In case previous runs are from another shell job table, kill by bound ports too.
for port in 18448 19998 19096; do
  pids="$(
    ss -ltnp 2>/dev/null \
      | grep -E "[\\.:]${port}[[:space:]]" \
      | sed -n 's/.*pid=\([0-9]\+\).*/\1/p' \
      | sort -u \
      || true
  )"
  if [[ -n "${pids}" ]]; then
    # shellcheck disable=SC2086
    kill -9 ${pids} >/dev/null 2>&1 || true
  fi
done
sleep 0.2

rm -f /tmp/arc_h2_backend.log /tmp/arc_h2_gateway.log /tmp/arc_h2_client_body.txt

python3 -m http.server 19096 --bind 127.0.0.1 >/tmp/arc_h2_backend.log 2>&1 &
BPID=$!

./target/release/arc-gateway --config tmp/tls_state_check/arc.json >/tmp/arc_h2_gateway.log 2>&1 &
APID=$!

cleanup() {
  kill "$APID" "$BPID" >/dev/null 2>&1 || true
  wait "$APID" "$BPID" 2>/dev/null || true
}
trap cleanup EXIT

sleep 1.0
echo "=== listeners ==="
ss -ltnp | grep -E '19096|18448|19998' || true

echo "=== backend probe ==="
curl -sS --connect-timeout 2 --max-time 4 \
  http://127.0.0.1:19096/ \
  -o /tmp/arc_h2_backend_probe.txt \
  -w "backend_status=%{http_code}\n" || true

curl -sk --http2 --connect-timeout 3 --max-time 8 \
  https://127.0.0.1:18448/ \
  -o /tmp/arc_h2_client_body.txt \
  -w "status=%{http_code}\n"

echo "=== gateway.log ==="
sed -n '1,260p' /tmp/arc_h2_gateway.log || true
echo "=== backend.log ==="
sed -n '1,200p' /tmp/arc_h2_backend.log || true
echo "=== body.size ==="
wc -c /tmp/arc_h2_client_body.txt || true
