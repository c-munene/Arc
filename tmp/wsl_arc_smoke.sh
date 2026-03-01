#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
export CARGO_HOME=/mnt/c/Users/win11/.cargo
export CARGO_NET_OFFLINE=true

cat > /tmp/arc_smoke_backend.py <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'arc-smoke-ok\n'
        self.send_response(200)
        self.send_header('Content-Type','text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, fmt, *args):
        return
HTTPServer(('127.0.0.1', 19000), H).serve_forever()
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

cleanup() {
  set +e
  if [ -n "${GW_PID:-}" ]; then kill "$GW_PID" >/dev/null 2>&1 || true; fi
  if [ -n "${BACK_PID:-}" ]; then kill "$BACK_PID" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

cargo run -p arc-gateway --offline -- --config /tmp/arc_smoke.json >/tmp/arc_smoke_gateway.log 2>&1 &
GW_PID=$!

sleep 3
HTTP_CODE=$(curl -sS -o /tmp/arc_smoke_resp.txt -w '%{http_code}' http://127.0.0.1:18080/)
BODY=$(cat /tmp/arc_smoke_resp.txt)

BACK_LOG=$(wc -c /tmp/arc_smoke_backend.log | awk '{print $1}')
GW_LOG=$(wc -c /tmp/arc_smoke_gateway.log | awk '{print $1}')

echo "HTTP_CODE=$HTTP_CODE"
echo "BODY=$BODY"
echo "BACK_LOG_BYTES=$BACK_LOG"
echo "GW_LOG_BYTES=$GW_LOG"
