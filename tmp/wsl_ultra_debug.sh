#!/usr/bin/env bash
set -euo pipefail
ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/wsl_ultra_debug"
mkdir -p "$WORK/root"
echo ok > "$WORK/root/x"
pkill -f "python3 -m http.server 3000" >/dev/null 2>&1 || true
pkill -f "target-ultra-wsl/release/ultra_gateway" >/dev/null 2>&1 || true
python3 -m http.server 3000 --bind 127.0.0.1 --directory "$WORK/root" > "$WORK/backend.out" 2> "$WORK/backend.err" &
echo $! > "$WORK/backend.pid"
"$ROOT/target-ultra-wsl/release/ultra_gateway" --listen 127.0.0.1:8083 --upstream 127.0.0.1:3000 --workers 2 --buf-count 1024 > "$WORK/ultra.out" 2> "$WORK/ultra.err" &
echo $! > "$WORK/ultra.pid"
sleep 2
set +e
curl -v --max-time 3 http://127.0.0.1:8083/x > "$WORK/curl.out" 2> "$WORK/curl.err"
echo CURL_RC:$? > "$WORK/curl.rc"
set -e
pgrep -af ultra_gateway > "$WORK/ps.ultra" || true
sleep 1
kill "$(cat "$WORK/ultra.pid")" "$(cat "$WORK/backend.pid")" >/dev/null 2>&1 || true
