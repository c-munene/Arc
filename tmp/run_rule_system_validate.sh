#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
TMP="$ROOT/tmp/rule_validate"
CFG="$TMP/arc_rules_validate.json"
BACK_PY="$TMP/backend.py"
GW_LOG="$TMP/gateway.log"
BK_LOG="$TMP/backend.log"
RESULT="$TMP/result.txt"
CERT="$ROOT/tmp/tls_smoke/cert.pem"
KEY="$ROOT/tmp/tls_smoke/key.pem"

mkdir -p "$TMP"
: >"$RESULT"

cleanup() {
  if [[ -n "${GW_PID:-}" ]]; then
    kill "$GW_PID" >/dev/null 2>&1 || true
    wait "$GW_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${BK_PID:-}" ]]; then
    kill "$BK_PID" >/dev/null 2>&1 || true
    wait "$BK_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

cat >"$BACK_PY" <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        host = self.headers.get("Host", "")
        body = f"backend-ok method=GET host={host} path={self.path}\n".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        host = self.headers.get("Host", "")
        length = int(self.headers.get("Content-Length", "0"))
        if length:
            self.rfile.read(length)
        body = f"backend-ok method=POST host={host} path={self.path}\n".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

HTTPServer(("127.0.0.1", 19080), H).serve_forever()
PY

cat >"$CFG" <<JSON
{
  "listen": "127.0.0.1:18443",
  "admin_listen": "127.0.0.1:19943",
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
    "up_conn": 3000,
    "up_handshake": 3000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "require_upstream_mtls": false,
  "upstreams": [
    {
      "name": "default",
      "addr": "127.0.0.1:19080",
      "keepalive": 64,
      "idle_ttl_ms": 30000
    }
  ],
  "plugins": [],
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [
      {
        "sni": "localhost",
        "cert_pem": "$CERT",
        "key_pem": "$KEY"
      }
    ],
    "sni_routes": []
  },
  "routes": [
    {
      "path": "/match",
      "upstream": "default",
      "priority": 100,
      "matchers": [
        { "type": "method", "methods": ["GET"] },
        { "type": "host", "hosts": ["localhost"] }
      ],
      "action": { "type": "respond", "status": 200, "body": "match-get-host" }
    },
    {
      "path": "/match",
      "upstream": "default",
      "priority": 90,
      "matchers": [
        { "type": "method", "methods": ["POST"] },
        { "type": "host", "hosts": ["localhost"] }
      ],
      "action": { "type": "respond", "status": 201, "body": "match-post-host" }
    },
    {
      "path": "/forward",
      "upstream": "default",
      "priority": 80,
      "matchers": [
        { "type": "method", "methods": ["GET"] },
        { "type": "host", "hosts": ["localhost"] }
      ],
      "action": { "type": "forward" }
    },
    {
      "path": "/prio",
      "upstream": "default",
      "priority": 200,
      "matchers": [
        { "type": "method", "methods": ["GET"] },
        { "type": "host", "hosts": ["localhost"] }
      ],
      "action": { "type": "respond", "status": 200, "body": "prio-high" }
    },
    {
      "path": "/prio",
      "upstream": "default",
      "priority": 10,
      "matchers": [
        { "type": "method", "methods": ["GET"] },
        { "type": "host", "hosts": ["localhost"] }
      ],
      "action": { "type": "respond", "status": 200, "body": "prio-low" }
    },
    {
      "path": "/respond-h1",
      "upstream": "default",
      "priority": 100,
      "matchers": [
        { "type": "h2", "enabled": false }
      ],
      "action": {
        "type": "respond",
        "status": 202,
        "body": "respond-h1-body",
        "headers": [
          { "name": "x-route-mode", "value": "h1" }
        ]
      }
    },
    {
      "path": "/respond-h2",
      "upstream": "default",
      "priority": 100,
      "matchers": [
        { "type": "h2", "enabled": true }
      ],
      "action": {
        "type": "respond",
        "status": 206,
        "body": "respond-h2-body",
        "headers": [
          { "name": "x-route-mode", "value": "h2" }
        ]
      }
    },
    {
      "path": "/amb",
      "upstream": "default",
      "priority": 50,
      "matchers": [
        { "type": "method", "methods": ["GET"] },
        { "type": "host", "hosts": ["localhost"] }
      ],
      "action": { "type": "respond", "status": 200, "body": "amb-a" }
    },
    {
      "path": "/amb",
      "upstream": "default",
      "priority": 50,
      "matchers": [
        { "type": "method", "methods": ["GET"] },
        { "type": "host", "hosts": ["localhost"] }
      ],
      "action": { "type": "respond", "status": 200, "body": "amb-b" }
    }
  ]
}
JSON

# Default behavior is to rebuild so validation always matches latest source.
# Set SKIP_BUILD=1 to reuse existing binary.
if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  (cd "$ROOT" && cargo build -p arc-gateway >/dev/null)
elif [[ ! -x "$ROOT/target/debug/arc-gateway" ]]; then
  echo "arc-gateway binary missing and SKIP_BUILD=1 set" | tee -a "$RESULT"
  exit 1
fi

python3 "$BACK_PY" >"$BK_LOG" 2>&1 &
BK_PID=$!

"$ROOT/target/debug/arc-gateway" --config "$CFG" >"$GW_LOG" 2>&1 &
GW_PID=$!

ready=0
for _ in $(seq 1 120); do
  if curl -ksS --http1.1 https://localhost:18443/match -H "Host: localhost" -o /dev/null >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.2
done
if [[ "$ready" != "1" ]]; then
  echo "gateway not ready" | tee -a "$RESULT"
  tail -n 120 "$GW_LOG" | tee -a "$RESULT"
  exit 1
fi

req_h1() {
  local name="$1"
  local expected_code="$2"
  local expected_body="$3"
  shift 3
  local body="$TMP/${name}.body"
  local hdr="$TMP/${name}.hdr"
  local code
  code=$(curl -ksS --http1.1 -D "$hdr" -o "$body" -w "%{http_code}" "$@")
  local one_line
  one_line=$(tr -d '\r' <"$body" | tr '\n' ' ')
  echo "$name code=$code body=$one_line" | tee -a "$RESULT"
  [[ "$code" == "$expected_code" ]] || { echo "$name FAIL code expected=$expected_code got=$code" | tee -a "$RESULT"; exit 1; }
  if [[ -n "$expected_body" ]]; then
    grep -F "$expected_body" "$body" >/dev/null || { echo "$name FAIL body missing: $expected_body" | tee -a "$RESULT"; exit 1; }
  fi
}

req_h2() {
  local name="$1"
  local expected_code="$2"
  local expected_body="$3"
  shift 3
  local body="$TMP/${name}.body"
  local hdr="$TMP/${name}.hdr"
  local out
  out=$(curl -ksS --http2 -D "$hdr" -o "$body" -w "%{http_code} %{http_version}" "$@")
  local code version
  code="$(echo "$out" | awk '{print $1}')"
  version="$(echo "$out" | awk '{print $2}')"
  local one_line
  one_line=$(tr -d '\r' <"$body" | tr '\n' ' ')
  echo "$name code=$code version=$version body=$one_line" | tee -a "$RESULT"
  [[ "$code" == "$expected_code" ]] || { echo "$name FAIL code expected=$expected_code got=$code" | tee -a "$RESULT"; exit 1; }
  [[ "$version" == "2" ]] || { echo "$name FAIL http_version expected=2 got=$version" | tee -a "$RESULT"; exit 1; }
  if [[ -n "$expected_body" ]]; then
    grep -F "$expected_body" "$body" >/dev/null || { echo "$name FAIL body missing: $expected_body" | tee -a "$RESULT"; exit 1; }
  fi
}

echo "[1] method + host" | tee -a "$RESULT"
req_h1 m1_get_host 200 "match-get-host" https://localhost:18443/match -H "Host: localhost"
req_h1 m1_post_host 201 "match-post-host" -X POST https://localhost:18443/match -H "Host: localhost"
req_h1 m1_host_miss 404 "" https://localhost:18443/match -H "Host: bad.local"
req_h1 m1_forward 200 "backend-ok method=GET host=localhost" https://localhost:18443/forward -H "Host: localhost"

echo "[2] priority" | tee -a "$RESULT"
req_h1 m2_priority 200 "prio-high" https://localhost:18443/prio -H "Host: localhost"

echo "[3] respond action" | tee -a "$RESULT"
req_h1 m3_h1_respond 202 "respond-h1-body" https://localhost:18443/respond-h1 -H "Host: localhost"
grep -i '^x-route-mode: h1' "$TMP/m3_h1_respond.hdr" >/dev/null || {
  echo "m3_h1_respond FAIL missing x-route-mode header" | tee -a "$RESULT"
  exit 1
}
req_h2 m3_h2_respond 206 "respond-h2-body" https://localhost:18443/respond-h2 -H "Host: localhost"

echo "[4] ambiguity 503" | tee -a "$RESULT"
req_h1 m4_ambiguous 503 "" https://localhost:18443/amb -H "Host: localhost"
req_h1 m4_no_panic 200 "match-get-host" https://localhost:18443/match -H "Host: localhost"

echo "ALL_CHECKS_PASSED" | tee -a "$RESULT"
echo "$RESULT"
