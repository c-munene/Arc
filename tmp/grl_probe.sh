#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
OUT="$ROOT/tmp/grl_dist_probe"
mkdir -p "$OUT"
cd "$ROOT"

REDIS="$ROOT/tmp/grl_dist_redis/redis-7.2.5/src/redis-server"
CLI="$ROOT/tmp/grl_dist_redis/redis-7.2.5/src/redis-cli"

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

cat > "$OUT/a.json" <<'JSON'
{
  "listen": "127.0.0.1:18181",
  "admin_listen": "127.0.0.1:19921",
  "workers": 1,
  "linger_ms": 300,
  "io_uring": { "entries": 1024, "accept_multishot": true, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false },
  "buffers": { "buf_size": 8192, "buf_count": 256 },
  "timeouts_ms": { "cli_handshake": 3000, "cli_read": 5000, "up_conn": 2000, "up_handshake": 3000, "up_write": 5000, "up_read": 5000, "cli_write": 5000 },
  "require_upstream_mtls": false,
  "global_rate_limit": {
    "backend": "redis",
    "redis": { "url": "redis://127.0.0.1:36379/0", "budget_ms": 2, "circuit_open_ms": 500 }
  },
  "upstreams": [{ "name": "default", "addr": "127.0.0.1:19180", "keepalive": 64, "idle_ttl_ms": 30000 }],
  "plugins": [],
  "routes": [{ "path": "/", "upstream": "default", "plugins": [], "rate_limit": { "rps": 100, "burst": 100 } }]
}
JSON

cp "$OUT/a.json" "$OUT/b.json"
sed -i 's/18181/18182/g; s/19921/19922/g' "$OUT/b.json"

python3 "$OUT/backend.py" >"$OUT/backend.log" 2>&1 &
BACK_PID=$!

"$REDIS" --port 36379 --save "" --appendonly no >"$OUT/redis.log" 2>&1 &
REDIS_PID=$!

cleanup() {
  set +e
  kill "${ARC_A_PID:-}" >/dev/null 2>&1 || true
  kill "${ARC_B_PID:-}" >/dev/null 2>&1 || true
  kill "$BACK_PID" >/dev/null 2>&1 || true
  kill "$REDIS_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 50); do
  if "$CLI" -p 36379 PING >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

"$CLI" -p 36379 FLUSHALL >/dev/null

./target/debug/arc-gateway --config "$OUT/a.json" >"$OUT/a.log" 2>&1 &
ARC_A_PID=$!
./target/debug/arc-gateway --config "$OUT/b.json" >"$OUT/b.log" 2>&1 &
ARC_B_PID=$!

for _ in $(seq 1 80); do
  if curl -fsS "http://127.0.0.1:18181/" >/dev/null 2>&1 && curl -fsS "http://127.0.0.1:18182/" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

python3 "$ROOT/tmp/global_rl_loadgen.py" \
  --targets "http://127.0.0.1:18181/,80;http://127.0.0.1:18182/,80" \
  --duration 8 \
  --workers-per-target 2 \
  --output "$OUT/s1.json"

"$CLI" -p 36379 KEYS "*" >"$OUT/redis_keys.txt"
for k in $("$CLI" -p 36379 KEYS "*"); do
  echo "KEY=$k" >>"$OUT/redis_keys.txt"
  "$CLI" -p 36379 HGETALL "$k" >>"$OUT/redis_keys.txt"
  echo "--" >>"$OUT/redis_keys.txt"
done

python3 - <<'PY'
import json
from pathlib import Path

p = Path("/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/grl_dist_probe/s1.json")
d = json.loads(p.read_text(encoding="utf-8"))
print(d["global"])
print(d["targets"])
PY
