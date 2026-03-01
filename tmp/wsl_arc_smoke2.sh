#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
export CARGO_HOME=/mnt/c/Users/win11/.cargo
export CARGO_NET_OFFLINE=true

cargo build -p arc-gateway --offline >/tmp/arc_smoke_build.log 2>&1

cat > /tmp/arc_smoke_backend.py <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"arc-smoke-ok\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

HTTPServer(("127.0.0.1", 19000), H).serve_forever()
PY

cat > /tmp/arc_smoke.json <<'JSON'
{
  "listen": "127.0.0.1:18080",
  "admin_listen": "127.0.0.1:19900",
  "workers": 1,
  "io_uring": {
    "entries": 1024,
    "accept_multishot": true,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "buffers": {
    "buf_size": 8192,
    "buf_count": 4096
  },
  "timeouts_ms": {
    "cli_read": 5000,
    "up_conn": 2000,
    "up_write": 5000,
    "up_read": 5000,
    "cli_write": 5000
  },
  "upstreams": [
    {
      "name": "default",
      "addr": "127.0.0.1:19000",
      "keepalive": 128,
      "idle_ttl_ms": 30000
    }
  ],
  "plugins": [],
  "routes": [
    {
      "path": "/",
      "upstream": "default",
      "plugins": [],
      "rate_limit": null
    }
  ]
}
JSON

python3 /tmp/arc_smoke_backend.py >/tmp/arc_smoke_backend.log 2>&1 &
BACK_PID=$!
GW_PID=""

cleanup() {
  set +e
  if [ -n "$GW_PID" ]; then kill "$GW_PID" >/dev/null 2>&1 || true; fi
  if [ -n "$BACK_PID" ]; then kill "$BACK_PID" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

./target/debug/arc-gateway --config /tmp/arc_smoke.json >/tmp/arc_smoke_gateway.log 2>&1 &
GW_PID=$!

ok=0
for i in $(seq 1 20); do
  if curl -sS -o /tmp/arc_smoke_resp.txt -w '%{http_code}' http://127.0.0.1:18080/ >/tmp/arc_smoke_code.txt 2>/tmp/arc_smoke_curl.err; then
    ok=1
    break
  fi
  sleep 1
done

if [ "$ok" -ne 1 ]; then
  echo "SMOKE_OK=0"
  echo "CURL_ERR_START"
  cat /tmp/arc_smoke_curl.err || true
  echo "CURL_ERR_END"
  echo "GW_LOG_TAIL_START"
  tail -n 120 /tmp/arc_smoke_gateway.log || true
  echo "GW_LOG_TAIL_END"
  exit 1
fi

http_code=$(cat /tmp/arc_smoke_code.txt)
body=$(cat /tmp/arc_smoke_resp.txt)
back_log_bytes=$(wc -c /tmp/arc_smoke_backend.log | awk '{print $1}')
gw_log_bytes=$(wc -c /tmp/arc_smoke_gateway.log | awk '{print $1}')

echo "SMOKE_OK=1"
echo "HTTP_CODE=$http_code"
echo "BODY=$body"
echo "BACK_LOG_BYTES=$back_log_bytes"
echo "GW_LOG_BYTES=$gw_log_bytes"
