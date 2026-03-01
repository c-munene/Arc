#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/error_pages_probe"
mkdir -p "$WORK"

cat >"$WORK/backend.py" <<'PY'
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path.endswith("/up502"):
            code, body = 502, b"upstream 502"
        elif path.endswith("/up504"):
            code, body = 504, b"upstream 504"
        elif path.endswith("/up404"):
            code, body = 404, b"upstream 404"
        elif path.endswith("/ok"):
            code, body = 200, b"ok"
        else:
            code, body = 200, b"default"

        self.send_response(code)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        return


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 19080), Handler).serve_forever()
PY

cat >"$WORK/error_service.py" <<'PY'
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path.startswith("/__arc/error/"):
            status = self.headers.get("X-Arc-Error-Status", "")
            source = self.headers.get("X-Arc-Error-Source", "")
            rid = self.headers.get("X-Arc-Request-Id", "")
            upstream = self.headers.get("X-Arc-Upstream", "")
            body = (
                f"error-service status={status} source={source} "
                f"request_id={rid} upstream={upstream}"
            ).encode("utf-8")
            code = 200
        else:
            body = b"error-service default"
            code = 200

        self.send_response(code)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        return


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 19081), Handler).serve_forever()
PY

cat >"$WORK/err5xx.html" <<'EOF'
<html><body>GLOBAL 5XX FILE</body></html>
EOF

WORK="$WORK" python3 - <<'PY'
import json
import os

work = os.environ["WORK"]
cfg = {
    "listen": "127.0.0.1:18081",
    "admin_listen": "127.0.0.1:19091",
    "workers": 1,
    "linger_ms": 100,
    "io_uring": {
        "entries": 256,
        "accept_multishot": False,
        "accept_prepost": 32,
        "tick_ms": 10,
        "sqpoll": False,
        "sqpoll_idle_ms": 0,
        "iopoll": False,
    },
    "buffers": {"buf_size": 8192, "buf_count": 512},
    "timeouts_ms": {
        "cli_handshake": 1000,
        "cli_read": 30000,
        "up_conn": 1000,
        "up_handshake": 1000,
        "up_write": 30000,
        "up_read": 30000,
        "cli_write": 30000,
    },
    "require_upstream_mtls": False,
    "upstreams": [
        {"name": "api-service", "addr": "127.0.0.1:19080", "keepalive": 64, "idle_ttl_ms": 30000},
        {"name": "error-page-service", "addr": "127.0.0.1:19081", "keepalive": 8, "idle_ttl_ms": 30000},
        {"name": "dead-service", "addr": "127.0.0.1:19999", "keepalive": 1, "idle_ttl_ms": 1000},
    ],
    "plugins": [],
    "routes": [
        {
            "path": "/api/*",
            "upstream": "api-service",
            "error_pages": {
                "502": {
                    "when": "upstream_error",
                    "body": "{\"error\":\"service_unavailable\",\"request_id\":\"$request_id\",\"upstream\":\"$upstream.name\"}",
                    "content_type": "application/json",
                },
                "504": {
                    "body": "{\"error\":\"timeout\",\"status\":\"$error.status\"}",
                    "content_type": "application/json",
                },
                "404": {"redirect": "/404.html", "code": 302},
            },
        },
        {
            "path": "/range/*",
            "upstream": "api-service",
            "error_pages": {
                "502-504": {
                    "body": "range:$error.status",
                    "content_type": "text/plain; charset=utf-8",
                }
            },
        },
        {
            "path": "/broken/*",
            "upstream": "dead-service",
            "error_pages": {
                "502": {
                    "when": "upstream_error",
                    "body": "should_not_match",
                    "content_type": "text/plain; charset=utf-8",
                }
            },
        },
    ],
    "defaults": {
        "error_pages": {
            "5xx": {"file": os.path.join(work, "err5xx.html")},
            "404": {"upstream": "error-page-service"},
        }
    },
}

with open(os.path.join(work, "arc_error_pages.json"), "w", encoding="utf-8") as f:
    json.dump(cfg, f, ensure_ascii=False, indent=2)
PY

cleanup() {
  set +e
  if [[ -n "${ARC_PID:-}" ]]; then kill "$ARC_PID" 2>/dev/null || true; fi
  if [[ -n "${BACK_PID:-}" ]]; then kill "$BACK_PID" 2>/dev/null || true; fi
  if [[ -n "${ERR_PID:-}" ]]; then kill "$ERR_PID" 2>/dev/null || true; fi
}
trap cleanup EXIT

cd "$ROOT"
cargo build -p arc-gateway >/dev/null

python3 "$WORK/backend.py" >"$WORK/backend.log" 2>&1 &
BACK_PID=$!
python3 "$WORK/error_service.py" >"$WORK/error_service.log" 2>&1 &
ERR_PID=$!
target/debug/arc-gateway --config "$WORK/arc_error_pages.json" >"$WORK/arc.log" 2>&1 &
ARC_PID=$!

for _ in $(seq 1 120); do
  if ! kill -0 "$ARC_PID" 2>/dev/null; then
    echo "ASSERT_FAIL: arc-gateway exited before ready"
    tail -n 120 "$WORK/arc.log" || true
    exit 1
  fi
  if curl -sS --max-time 1 "http://127.0.0.1:18081/api/ok" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

assert_eq() {
  local got="$1"
  local want="$2"
  local msg="$3"
  if [[ "$got" != "$want" ]]; then
    echo "ASSERT_FAIL: $msg (got=$got want=$want)"
    exit 1
  fi
}

assert_contains() {
  local file="$1"
  local pattern="$2"
  local msg="$3"
  if ! grep -Eq "$pattern" "$file"; then
    echo "ASSERT_FAIL: $msg (pattern=$pattern file=$file)"
    cat "$file"
    exit 1
  fi
}

echo "== CASE 1: route 502 upstream_error + template =="
code1=$(curl -sS -o "$WORK/case1.body" -w "%{http_code}" "http://127.0.0.1:18081/api/up502")
assert_eq "$code1" "502" "case1 status"
assert_contains "$WORK/case1.body" '"error":"service_unavailable"' "case1 body error"
assert_contains "$WORK/case1.body" '"request_id":"[0-9a-f]+"' "case1 request_id variable"
assert_contains "$WORK/case1.body" '"upstream":"api-service"' "case1 upstream variable"

echo "== CASE 2: route 404 redirect =="
code2=$(curl -sS -D "$WORK/case2.head" -o /dev/null -w "%{http_code}" "http://127.0.0.1:18081/api/up404")
assert_eq "$code2" "302" "case2 status"
assert_contains "$WORK/case2.head" '^[Ll]ocation: /404\.html' "case2 redirect location"

echo "== CASE 3: route range 502-504 =="
code3=$(curl -sS -o "$WORK/case3.body" -w "%{http_code}" "http://127.0.0.1:18081/range/up504")
assert_eq "$code3" "504" "case3 status"
assert_contains "$WORK/case3.body" '^range:504$' "case3 range body"

echo "== CASE 4: gateway 502 fallback to default 5xx file =="
code4=$(curl -sS -o "$WORK/case4.body" -w "%{http_code}" "http://127.0.0.1:18081/broken/x")
assert_eq "$code4" "502" "case4 status"
assert_contains "$WORK/case4.body" 'GLOBAL 5XX FILE' "case4 default file body"

echo "== CASE 5: no-route 404 -> default upstream action =="
code5=$(curl -sS -o "$WORK/case5.body" -w "%{http_code}" "http://127.0.0.1:18081/no-route")
assert_eq "$code5" "200" "case5 status"
assert_contains "$WORK/case5.body" 'error-service status=404 source=gateway' "case5 default upstream action"

{
  echo "case1_status=$code1"
  echo "case1_body=$(tr -d '\n' < "$WORK/case1.body")"
  echo "case2_status=$code2"
  echo "case2_location=$(grep -i '^Location:' "$WORK/case2.head" | tr -d '\r')"
  echo "case3_status=$code3"
  echo "case3_body=$(tr -d '\n' < "$WORK/case3.body")"
  echo "case4_status=$code4"
  echo "case4_body=$(tr -d '\n' < "$WORK/case4.body")"
  echo "case5_status=$code5"
  echo "case5_body=$(tr -d '\n' < "$WORK/case5.body")"
} >"$WORK/result.txt"

echo "OK: error_pages validation passed"
echo "Result file: $WORK/result.txt"
