#!/usr/bin/env bash
set -euo pipefail
ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
TMP="$ROOT/tmp/p1_smoke"
CFG="$TMP/arc.json"
B1="$TMP/b1.py"
B2="$TMP/b2.py"
GWLOG="$TMP/gw.log"
B1LOG="$TMP/b1.log"
B2LOG="$TMP/b2.log"
RES="$TMP/result.txt"

mkdir -p "$TMP"
: > "$RES"

cleanup() {
  for p in ${GW_PID:-} ${B1_PID:-} ${B2_PID:-}; do
    if [[ -n "${p:-}" ]]; then
      kill "$p" >/dev/null 2>&1 || true
      wait "$p" >/dev/null 2>&1 || true
    fi
  done
}
trap cleanup EXIT

cat > "$B1" << 'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        xr = self.headers.get('x-route', '')
        body = f"u1 path={self.path} x-route={xr}\\n".encode()
        self.send_response(200)
        self.send_header('Content-Type','text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        return
HTTPServer(('127.0.0.1', 19080), H).serve_forever()
PY

cat > "$B2" << 'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        xr = self.headers.get('x-route', '')
        body = f"u2 path={self.path} x-route={xr}\\n".encode()
        self.send_response(200)
        self.send_header('Content-Type','text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        return
HTTPServer(('127.0.0.1', 19081), H).serve_forever()
PY

cat > "$CFG" << 'JSON'
{
  "listen": "127.0.0.1:18480",
  "admin_listen": "127.0.0.1:19980",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {
    "entries": 1024,
    "accept_multishot": true,
    "accept_prepost": 16,
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
    "cli_handshake": 3000,
    "cli_read": 30000,
    "up_conn": 1000,
    "up_handshake": 3000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "require_upstream_mtls": false,
  "upstreams": [
    {"name":"u1","addr":"127.0.0.1:19080","keepalive":64,"idle_ttl_ms":30000},
    {"name":"u2","addr":"127.0.0.1:19081","keepalive":64,"idle_ttl_ms":30000},
    {"name":"down","addr":"127.0.0.1:19082","keepalive":64,"idle_ttl_ms":30000}
  ],
  "plugins": [],
  "routes": [
    {
      "path":"/rw/*",
      "upstream":"u1",
      "rewrite":{"pattern":"^/rw","replace":"/real"},
      "headers":[{"op":"set","name":"x-route","value":"p1"}],
      "retry":{"max_retries":1,"backoff":"1ms","idempotent_only":true},
      "action":{"type":"forward"}
    },
    {
      "path":"/ab",
      "split":{
        "key":{"type":"random"},
        "choices":[
          {"upstream":"u1","weight":8},
          {"upstream":"u2","weight":2}
        ]
      },
      "retry":{"max_retries":0,"backoff":"1ms","idempotent_only":true},
      "action":{"type":"forward"}
    },
    {
      "path":"/retry3",
      "split":{
        "key":{"type":"path"},
        "choices":[
          {"upstream":"down","weight":1},
          {"upstream":"u1","weight":1}
        ]
      },
      "retry":{"max_retries":1,"backoff":"1ms","idempotent_only":true},
      "action":{"type":"forward"}
    }
  ]
}
JSON

python3 "$B1" > "$B1LOG" 2>&1 &
B1_PID=$!
python3 "$B2" > "$B2LOG" 2>&1 &
B2_PID=$!

"$ROOT/target/debug/arc-gateway" --config "$CFG" > "$GWLOG" 2>&1 &
GW_PID=$!

ready=0
for _ in $(seq 1 100); do
  if curl -sS "http://127.0.0.1:18480/ab" -o /dev/null >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
if [[ "$ready" != "1" ]]; then
  echo "gateway not ready" | tee -a "$RES"
  tail -n 80 "$GWLOG" | tee -a "$RES"
  exit 1
fi

RW=$(curl -sS "http://127.0.0.1:18480/rw/abc" -H "Host: t.local")
echo "rewrite_resp=$RW" | tee -a "$RES"
if ! echo "$RW" | grep -q "path=/real/abc"; then
  echo "FAIL rewrite path" | tee -a "$RES"; exit 1
fi
if ! echo "$RW" | grep -q "x-route=p1"; then
  echo "FAIL header mutation" | tee -a "$RES"; exit 1
fi

U1=0; U2=0
for _ in $(seq 1 120); do
  R=$(curl -sS "http://127.0.0.1:18480/ab")
  if echo "$R" | grep -q "^u1"; then U1=$((U1+1)); fi
  if echo "$R" | grep -q "^u2"; then U2=$((U2+1)); fi
done
echo "split_counts u1=$U1 u2=$U2" | tee -a "$RES"
if [[ "$U1" -eq 0 || "$U2" -eq 0 ]]; then
  echo "FAIL split distribution" | tee -a "$RES"; exit 1
fi

RR=$(curl -sS -w " code=%{http_code}" "http://127.0.0.1:18480/retry3")
echo "retry_resp=$RR" | tee -a "$RES"
if ! echo "$RR" | grep -q "code=200"; then
  echo "FAIL retry status" | tee -a "$RES"; exit 1
fi
if ! echo "$RR" | grep -q "^u1"; then
  echo "FAIL retry upstream switch" | tee -a "$RES"; exit 1
fi

echo "P1_SMOKE_OK" | tee -a "$RES"
echo "$RES"