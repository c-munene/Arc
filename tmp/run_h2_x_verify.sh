#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

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

mkdir -p tmp/h2_x_verify_root
printf 'REAL_BACKEND_X\n' > tmp/h2_x_verify_root/x

python3 -m http.server 19096 --bind 127.0.0.1 --directory tmp/h2_x_verify_root >/tmp/h2_x_verify_backend.log 2>&1 &
BPID=$!
./target/release/arc-gateway --config tmp/tls_real_backend/arc.json >/tmp/h2_x_verify_gateway.log 2>&1 &
GPID=$!

cleanup() {
  kill "$GPID" "$BPID" >/dev/null 2>&1 || true
  wait "$GPID" "$BPID" 2>/dev/null || true
}
trap cleanup EXIT

sleep 1.0
curl -sk --http2 --connect-timeout 3 --max-time 8 https://127.0.0.1:18448/x -o /tmp/h2_x_verify_body.txt -w "status=%{http_code}\n"
echo -n "body="
tr -d '\r\n' </tmp/h2_x_verify_body.txt
echo
