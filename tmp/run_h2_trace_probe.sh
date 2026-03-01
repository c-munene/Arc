#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
rm -rf /tmp/arc_h2_trace_probe
mkdir -p /tmp/arc_h2_trace_probe
cat > /tmp/arc_h2_trace_probe/backend.py <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        tp = self.headers.get('traceparent', '')
        body = tp.encode('utf-8')
        self.send_response(200)
        self.send_header('Content-Type', 'text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, fmt, *args):
        return
HTTPServer(('127.0.0.1', 19098), H).serve_forever()
PY
cat > /tmp/arc_h2_trace_probe/arc.json <<'JSON'
{
  "listen": "127.0.0.1:19450",
  "admin_listen": "127.0.0.1:19999",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 1024, "accept_multishot": false, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 2048},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 30000, "up_conn": 3000, "up_handshake": 1000, "up_write": 30000, "up_read": 30000, "cli_write": 30000},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19098", "keepalive": 32, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [{"sni": "localhost", "cert_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem", "key_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem"}],
    "sni_routes": []
  },
  "logging": {
    "access": {"sample": 1.0},
    "output": {"file": "/tmp/arc_h2_trace_probe/access.log", "stdout": false}
  }
}
JSON
for p in 19098 19450 19999; do
  pids="$(ss -ltnp 2>/dev/null | grep -E "[\\.:]${p}[[:space:]]" | sed -n 's/.*pid=\([0-9]\+\).*/\1/p' | sort -u || true)"
  if [[ -n "$pids" ]]; then
    kill -9 $pids >/dev/null 2>&1 || true
  fi
done
python3 /tmp/arc_h2_trace_probe/backend.py >/tmp/arc_h2_trace_probe/backend.log 2>&1 &
BPID=$!
./target/debug/arc-gateway --config /tmp/arc_h2_trace_probe/arc.json >/tmp/arc_h2_trace_probe/arc.log 2>&1 &
GPID=$!
cleanup(){ kill "$GPID" "$BPID" >/dev/null 2>&1 || true; wait "$GPID" "$BPID" >/dev/null 2>&1 || true; }
trap cleanup EXIT
sleep 1
resp1="$(curl -sk --noproxy '*' --http2 https://localhost:19450/)"
resp2="$(curl -sk --noproxy '*' --http2 -H 'traceparent: 00-0123456789abcdef0123456789abcdef-1111111111111111-01' https://localhost:19450/)"
echo "RESP1=$resp1"
echo "RESP2=$resp2"
sleep 1
wc -l /tmp/arc_h2_trace_probe/access.log
cat /tmp/arc_h2_trace_probe/access.log
