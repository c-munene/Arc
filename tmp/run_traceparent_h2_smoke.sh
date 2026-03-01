#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

WORK=/tmp/arc_h2_trace_smoke
mkdir -p "$WORK"
rm -f "$WORK"/access.log "$WORK"/backend.log "$WORK"/arc.log

cat > "$WORK/backend.py" <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        tp = self.headers.get("traceparent", "")
        body = tp.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

HTTPServer(("127.0.0.1", 19098), H).serve_forever()
PY

cat > "$WORK/arc.json" <<'JSON'
{
  "listen": "127.0.0.1:19450",
  "admin_listen": "127.0.0.1:19999",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {
    "entries": 1024,
    "accept_multishot": false,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "buffers": {
    "buf_size": 8192,
    "buf_count": 2048
  },
  "timeouts_ms": {
    "cli_handshake": 1000,
    "cli_read": 30000,
    "up_conn": 3000,
    "up_handshake": 1000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "require_upstream_mtls": false,
  "upstreams": [
    {
      "name": "u",
      "addr": "127.0.0.1:19098",
      "keepalive": 32,
      "idle_ttl_ms": 30000
    }
  ],
  "plugins": [],
  "routes": [
    {
      "path": "/",
      "upstream": "u",
      "plugins": [],
      "rate_limit": null
    }
  ],
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [
      {
        "sni": "localhost",
        "cert_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem",
        "key_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem"
      }
    ],
    "sni_routes": []
  },
  "logging": {
    "access": {
      "sample": 1.0
    },
    "output": {
      "file": "/tmp/arc_h2_trace_smoke/access.log",
      "stdout": false
    }
  }
}
JSON

for port in 19098 19450 19999; do
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

python3 "$WORK/backend.py" >"$WORK/backend.log" 2>&1 &
BPID=$!
./target/debug/arc-gateway --config "$WORK/arc.json" >"$WORK/arc.log" 2>&1 &
GPID=$!

cleanup() {
  kill "$GPID" "$BPID" >/dev/null 2>&1 || true
  wait "$GPID" "$BPID" 2>/dev/null || true
}
trap cleanup EXIT

sleep 1

AUTO_RESP="$(curl -sk --noproxy '*' --http2 https://localhost:19450/)"
PASS_RESP="$(
  curl -sk --noproxy '*' --http2 \
    -H 'traceparent: 00-0123456789abcdef0123456789abcdef-1111111111111111-01' \
    https://localhost:19450/
)"

echo "AUTO=${AUTO_RESP}"
echo "PASS=${PASS_RESP}"

if [[ -z "${AUTO_RESP}" ]]; then
  echo "FAIL:auto response empty"
  exit 1
fi
if ! echo "${AUTO_RESP}" | grep -Eq '^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$'; then
  echo "FAIL:auto traceparent format invalid"
  exit 1
fi
if ! echo "${PASS_RESP}" | grep -Eq '^00-0123456789abcdef0123456789abcdef-[0-9a-f]{16}-[0-9a-f]{2}$'; then
  echo "FAIL:pass-through trace_id not preserved"
  exit 1
fi

AUTO_TRACE_ID="$(echo "${AUTO_RESP}" | sed -E 's/^00-([0-9a-f]{32})-.*/\1/')"

sleep 1

LINES="$(wc -l < "$WORK/access.log" | tr -d ' ')"
echo "LOG_LINES=${LINES}"
if [[ "${LINES}" -lt 2 ]]; then
  echo "FAIL:access log lines < 2"
  exit 1
fi

if ! grep -q "\"trace_id\":\"${AUTO_TRACE_ID}\"" "$WORK/access.log"; then
  echo "FAIL:auto trace_id missing in access log"
  exit 1
fi
if ! grep -q "\"trace_id\":\"0123456789abcdef0123456789abcdef\"" "$WORK/access.log"; then
  echo "FAIL:pass-through trace_id missing in access log"
  exit 1
fi

echo "TRACE_H2_SMOKE=OK"
