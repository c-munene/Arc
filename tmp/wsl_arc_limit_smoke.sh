#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
export CARGO_HOME=/mnt/c/Users/win11/.cargo
export CARGO_NET_OFFLINE=true

cargo build -p arc-gateway --offline >/tmp/arc_limit_build.log 2>&1

cat > /tmp/arc_limit_backend.py <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'limit-ok\n'
        self.send_response(200)
        self.send_header('Content-Type','text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, fmt, *args):
        return
HTTPServer(('127.0.0.1', 19100), H).serve_forever()
PY

cat > /tmp/arc_limit.json <<'JSON'
{
  "listen": "127.0.0.1:18180",
  "admin_listen": "127.0.0.1:19910",
  "workers": 1,
  "linger_ms": 300,
  "io_uring": {"entries": 1024, "accept_multishot": true, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 4096},
  "timeouts_ms": {"cli_read": 5000, "up_conn": 2000, "up_write": 5000, "up_read": 5000, "cli_write": 5000},
  "upstreams": [{"name": "default", "addr": "127.0.0.1:19100", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "default", "plugins": [], "rate_limit": {"rps": 1, "burst": 1}}]
}
JSON

python3 /tmp/arc_limit_backend.py >/tmp/arc_limit_backend.log 2>&1 &
BACK_PID=$!
GW_PID=""
cleanup(){
  set +e
  if [ -n "$GW_PID" ]; then kill "$GW_PID" >/dev/null 2>&1 || true; fi
  if [ -n "$BACK_PID" ]; then kill "$BACK_PID" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

./target/debug/arc-gateway --config /tmp/arc_limit.json >/tmp/arc_limit_gateway.log 2>&1 &
GW_PID=$!

for i in $(seq 1 20); do
  if curl -sS http://127.0.0.1:18180/ >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

codes=""
for i in $(seq 1 12); do
  c=$(curl -sS -o /dev/null -w '%{http_code}' http://127.0.0.1:18180/)
  codes="$codes $c"
  sleep 0.03
done

echo "CODES=$codes"
count_429=$(echo "$codes" | tr ' ' '\n' | grep -c '^429$' || true)
count_200=$(echo "$codes" | tr ' ' '\n' | grep -c '^200$' || true)
echo "COUNT_200=$count_200"
echo "COUNT_429=$count_429"
