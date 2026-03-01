#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
OUT="$ROOT/tmp/grl_inmem_probe"
mkdir -p "$OUT"
cd "$ROOT"

cat > "$OUT/backend.py" <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        b = b"ok\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def log_message(self, *args):
        return

HTTPServer(("127.0.0.1", 19180), H).serve_forever()
PY

cat > "$OUT/arc.json" <<'JSON'
{
  "listen": "127.0.0.1:18181",
  "admin_listen": "127.0.0.1:19921",
  "workers": 1,
  "linger_ms": 300,
  "io_uring": { "entries": 1024, "accept_multishot": true, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false },
  "buffers": { "buf_size": 8192, "buf_count": 256 },
  "timeouts_ms": { "cli_handshake": 3000, "cli_read": 5000, "up_conn": 2000, "up_handshake": 3000, "up_write": 5000, "up_read": 5000, "cli_write": 5000 },
  "require_upstream_mtls": false,
  "global_rate_limit": { "backend": "in_memory" },
  "upstreams": [{ "name": "default", "addr": "127.0.0.1:19180", "keepalive": 64, "idle_ttl_ms": 30000 }],
  "plugins": [],
  "routes": [{ "path": "/", "upstream": "default", "plugins": [], "rate_limit": { "rps": 100, "burst": 100 } }]
}
JSON

python3 "$OUT/backend.py" >"$OUT/backend.log" 2>&1 &
BACK_PID=$!
./target/debug/arc-gateway --config "$OUT/arc.json" >"$OUT/arc.log" 2>&1 &
ARC_PID=$!

cleanup() {
  set +e
  kill "$ARC_PID" >/dev/null 2>&1 || true
  kill "$BACK_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 80); do
  if curl -fsS "http://127.0.0.1:18181/" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

python3 "$ROOT/tmp/global_rl_loadgen.py" \
  --targets "http://127.0.0.1:18181/,200" \
  --duration 30 \
  --workers-per-target 4 \
  --output "$OUT/inmem.json"

python3 - <<'PY'
import json
from pathlib import Path

p = Path("/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/grl_inmem_probe/inmem.json")
d = json.loads(p.read_text(encoding="utf-8"))
g = d["global"]
print("sent", g["sent"], "2xx", g["2xx"], "429", g["429"], "errors", g["errors"])
print("2xx_rps", g["2xx"] / 30.0, "429_rps", g["429"] / 30.0)
PY
