#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
export CARGO_HOME=/mnt/c/Users/win11/.cargo
export CARGO_NET_OFFLINE=true

cargo build -p arc-gateway --offline >/tmp/arc_wrk_build.log 2>&1

cat > /tmp/arc_wrk_backend.py <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'wrk-ok\n'
        self.send_response(200)
        self.send_header('Content-Type','text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, fmt, *args):
        return
HTTPServer(('127.0.0.1', 19000), H).serve_forever()
PY

cat > /tmp/arc_wrk.json <<'JSON'
{
  "listen": "127.0.0.1:18080",
  "admin_listen": "127.0.0.1:19900",
  "workers": 1,
  "io_uring": {"entries": 1024, "accept_multishot": true, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 4096},
  "timeouts_ms": {"cli_read": 5000, "up_conn": 2000, "up_write": 5000, "up_read": 5000, "cli_write": 5000},
  "upstreams": [{"name": "default", "addr": "127.0.0.1:19000", "keepalive": 128, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "default", "plugins": [], "rate_limit": null}]
}
JSON

python3 /tmp/arc_wrk_backend.py >/tmp/arc_wrk_backend.log 2>&1 &
BACK_PID=$!
GW_PID=""
cleanup() {
  set +e
  if [ -n "$GW_PID" ]; then kill "$GW_PID" >/dev/null 2>&1 || true; fi
  if [ -n "$BACK_PID" ]; then kill "$BACK_PID" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

./target/debug/arc-gateway --config /tmp/arc_wrk.json >/tmp/arc_wrk_gateway.log 2>&1 &
GW_PID=$!

for i in $(seq 1 20); do
  if curl -sS http://127.0.0.1:18080/ >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

wrk -t4 -c100 -d10s http://127.0.0.1:19000/ > /tmp/arc_wrk_direct.txt
wrk -t4 -c100 -d10s http://127.0.0.1:18080/ > /tmp/arc_wrk_gateway.txt

echo "DIRECT_SUMMARY_START"
grep -E "Latency|Req/Sec|Requests/sec|Socket errors|Transfer/sec" /tmp/arc_wrk_direct.txt || true
echo "DIRECT_SUMMARY_END"

echo "GATEWAY_SUMMARY_START"
grep -E "Latency|Req/Sec|Requests/sec|Socket errors|Transfer/sec" /tmp/arc_wrk_gateway.txt || true
echo "GATEWAY_SUMMARY_END"
