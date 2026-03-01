#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
OUT_DIR="$ROOT/tmp/grl_dist_redis"
BACKEND_PORT=19180
ARC_A_PORT=18181
ARC_B_PORT=18182
ARC_A_ADMIN=19921
ARC_B_ADMIN=19922
REDIS_VER="7.2.5"
REDIS_BASE="$OUT_DIR/redis-$REDIS_VER"
REDIS_TAR="$OUT_DIR/redis-$REDIS_VER.tar.gz"
REDIS_URL_1="https://download.redis.io/releases/redis-$REDIS_VER.tar.gz"
REDIS_URL_2="https://mirrors.tuna.tsinghua.edu.cn/redis/redis-$REDIS_VER.tar.gz"

mkdir -p "$OUT_DIR"
cd "$ROOT"

REDIS_PORT="$(
python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"

cleanup() {
  set +e
  if [ -n "${LOAD_PID:-}" ]; then kill "$LOAD_PID" >/dev/null 2>&1 || true; fi
  if [ -n "${ARC_A_PID:-}" ]; then kill "$ARC_A_PID" >/dev/null 2>&1 || true; fi
  if [ -n "${ARC_B_PID:-}" ]; then kill "$ARC_B_PID" >/dev/null 2>&1 || true; fi
  if [ -n "${BACK_PID:-}" ]; then kill "$BACK_PID" >/dev/null 2>&1 || true; fi
  if [ -n "${REDIS_PID:-}" ]; then kill "$REDIS_PID" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

echo "[1/8] build arc-gateway"
export CARGO_HOME=/mnt/c/Users/win11/.cargo
if ! CARGO_NET_OFFLINE=true cargo build -p arc-gateway --offline >/dev/null 2>&1; then
  cargo build -p arc-gateway >/dev/null
fi

echo "[2/8] prepare redis binary (linux in WSL)"
if [ ! -x "$REDIS_BASE/src/redis-server" ]; then
  rm -rf "$REDIS_BASE" "$OUT_DIR/redis-$REDIS_VER"
  if [ ! -f "$REDIS_TAR" ]; then
    if ! curl -fsSL "$REDIS_URL_1" -o "$REDIS_TAR"; then
      if ! curl -fsSL "$REDIS_URL_2" -o "$REDIS_TAR"; then
        powershell.exe -NoProfile -Command \
          "\$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri '$REDIS_URL_1' -OutFile 'C:\\Users\\Shuakami_Projects\\CommunityProject\\Arc\\tmp\\grl_dist_redis\\redis-$REDIS_VER.tar.gz'" \
          >/dev/null
      fi
    fi
  fi
  tar -C "$OUT_DIR" -xzf "$REDIS_TAR"
  make -C "$REDIS_BASE" -j"$(nproc)" MALLOC=libc >/dev/null
fi
REDIS_SERVER="$REDIS_BASE/src/redis-server"
REDIS_CLI="$REDIS_BASE/src/redis-cli"

cat > "$OUT_DIR/backend.py" <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"ok\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, fmt, *args):
        return

HTTPServer(("127.0.0.1", 19180), H).serve_forever()
PY

cat > "$OUT_DIR/arc_a.json" <<JSON
{
  "listen": "127.0.0.1:$ARC_A_PORT",
  "admin_listen": "127.0.0.1:$ARC_A_ADMIN",
  "workers": 1,
  "linger_ms": 300,
  "io_uring": {"entries": 1024, "accept_multishot": true, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 256},
  "timeouts_ms": {"cli_handshake": 3000, "cli_read": 5000, "up_conn": 2000, "up_handshake": 3000, "up_write": 5000, "up_read": 5000, "cli_write": 5000},
  "require_upstream_mtls": false,
  "global_rate_limit": {
    "backend": "redis",
    "redis": {
      "url": "redis://127.0.0.1:$REDIS_PORT/0",
      "budget_ms": 2,
      "circuit_open_ms": 500,
      "prefetch": 128,
      "low_watermark": 16,
      "refill_backoff_ms": 1
    }
  },
  "upstreams": [{"name": "default", "addr": "127.0.0.1:$BACKEND_PORT", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "default", "plugins": [], "rate_limit": {"rps": 100, "burst": 100}}]
}
JSON

cat > "$OUT_DIR/arc_b.json" <<JSON
{
  "listen": "127.0.0.1:$ARC_B_PORT",
  "admin_listen": "127.0.0.1:$ARC_B_ADMIN",
  "workers": 1,
  "linger_ms": 300,
  "io_uring": {"entries": 1024, "accept_multishot": true, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 256},
  "timeouts_ms": {"cli_handshake": 3000, "cli_read": 5000, "up_conn": 2000, "up_handshake": 3000, "up_write": 5000, "up_read": 5000, "cli_write": 5000},
  "require_upstream_mtls": false,
  "global_rate_limit": {
    "backend": "redis",
    "redis": {
      "url": "redis://127.0.0.1:$REDIS_PORT/0",
      "budget_ms": 2,
      "circuit_open_ms": 500,
      "prefetch": 128,
      "low_watermark": 16,
      "refill_backoff_ms": 1
    }
  },
  "upstreams": [{"name": "default", "addr": "127.0.0.1:$BACKEND_PORT", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "default", "plugins": [], "rate_limit": {"rps": 100, "burst": 100}}]
}
JSON

start_redis() {
  "$REDIS_SERVER" --port "$REDIS_PORT" --save "" --appendonly no >"$OUT_DIR/redis.log" 2>&1 &
  REDIS_PID=$!
  for _ in $(seq 1 50); do
    if "$REDIS_CLI" -p "$REDIS_PORT" PING >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  echo "redis startup failed" >&2
  return 1
}

echo "[3/8] start backend + redis + 2x arc"
python3 "$OUT_DIR/backend.py" >"$OUT_DIR/backend.log" 2>&1 &
BACK_PID=$!

start_redis
"$REDIS_CLI" -p "$REDIS_PORT" FLUSHALL >/dev/null

./target/debug/arc-gateway --config "$OUT_DIR/arc_a.json" >"$OUT_DIR/arc_a.log" 2>&1 &
ARC_A_PID=$!
./target/debug/arc-gateway --config "$OUT_DIR/arc_b.json" >"$OUT_DIR/arc_b.log" 2>&1 &
ARC_B_PID=$!

for _ in $(seq 1 80); do
  if curl -fsS "http://127.0.0.1:$ARC_A_PORT/" >/dev/null 2>&1 && curl -fsS "http://127.0.0.1:$ARC_B_PORT/" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

echo "[4/8] scenario1: uniform load (80 + 80 rps, 30s)"
"$REDIS_CLI" -p "$REDIS_PORT" FLUSHALL >/dev/null
python3 "$ROOT/tmp/global_rl_loadgen.py" \
  --targets "http://127.0.0.1:$ARC_A_PORT/,80;http://127.0.0.1:$ARC_B_PORT/,80" \
  --duration 30 \
  --workers-per-target 4 \
  --output "$OUT_DIR/scenario1_uniform.json"

echo "[5/8] scenario2: single-side burst (200 + 0 rps, 30s)"
"$REDIS_CLI" -p "$REDIS_PORT" FLUSHALL >/dev/null
python3 "$ROOT/tmp/global_rl_loadgen.py" \
  --targets "http://127.0.0.1:$ARC_A_PORT/,200" \
  --duration 30 \
  --workers-per-target 4 \
  --output "$OUT_DIR/scenario2_single_burst.json"

echo "[6/8] scenario3: redis failure + auto-recover (80 + 80 rps, 36s)"
"$REDIS_CLI" -p "$REDIS_PORT" FLUSHALL >/dev/null
python3 "$ROOT/tmp/global_rl_loadgen.py" \
  --targets "http://127.0.0.1:$ARC_A_PORT/,80;http://127.0.0.1:$ARC_B_PORT/,80" \
  --duration 36 \
  --workers-per-target 4 \
  --output "$OUT_DIR/scenario3_failover_raw.json" &
LOAD_PID=$!

sleep 12
kill "$REDIS_PID" >/dev/null 2>&1 || true
wait_for_down=0
for _ in $(seq 1 20); do
  if ! "$REDIS_CLI" -p "$REDIS_PORT" PING >/dev/null 2>&1; then
    wait_for_down=1
    break
  fi
  sleep 0.1
done
echo "redis_down_confirmed=$wait_for_down" >"$OUT_DIR/scenario3_redis_events.txt"
echo "redis_down_at_s=12" >>"$OUT_DIR/scenario3_redis_events.txt"

sleep 12
start_redis
echo "redis_up_at_s=24" >>"$OUT_DIR/scenario3_redis_events.txt"

wait "$LOAD_PID"
unset LOAD_PID

echo "[7/8] summarize results"
python3 - "$OUT_DIR" <<'PY'
import json
import os
import sys

out = sys.argv[1]

def load(name):
    with open(os.path.join(out, name), "r", encoding="utf-8") as f:
        return json.load(f)

def sec_sum(timeline, sec_from, sec_to, key):
    total = 0
    for row in timeline:
        s = int(row.get("sec", -1))
        if sec_from <= s <= sec_to:
            total += int(row.get(key, 0))
    return total

def make_basic(src):
    g = src["global"]
    return {
        "sent": g["sent"],
        "achieved_rps": g["achieved_rps"],
        "2xx": g.get("2xx", 0),
        "429": g.get("429", 0),
        "errors": g.get("errors", 0),
        "status": g.get("status", {}),
        "targets": src.get("targets", []),
    }

s1 = load("scenario1_uniform.json")
s2 = load("scenario2_single_burst.json")
s3 = load("scenario3_failover_raw.json")

timeline = s3.get("timeline_global", [])
s3_pre = {
    "window_s": "0-11",
    "2xx": sec_sum(timeline, 0, 11, "200"),
    "429": sec_sum(timeline, 0, 11, "429"),
    "errors": sec_sum(timeline, 0, 11, "error"),
}
s3_fail = {
    "window_s": "12-23",
    "2xx": sec_sum(timeline, 12, 23, "200"),
    "429": sec_sum(timeline, 12, 23, "429"),
    "errors": sec_sum(timeline, 12, 23, "error"),
}
s3_recover = {
    "window_s": "24-35",
    "2xx": sec_sum(timeline, 24, 35, "200"),
    "429": sec_sum(timeline, 24, 35, "429"),
    "errors": sec_sum(timeline, 24, 35, "error"),
}

for row in (s3_pre, s3_fail, s3_recover):
    dur = 12.0
    row["2xx_rps"] = row["2xx"] / dur
    row["429_rps"] = row["429"] / dur

summary = {
    "scenario1_uniform_80_80": make_basic(s1),
    "scenario2_single_200_0": make_basic(s2),
    "scenario3_redis_failure_80_80": {
        "global": make_basic(s3),
        "windows": {
            "pre_redis_down": s3_pre,
            "redis_down_fallback": s3_fail,
            "post_redis_recover": s3_recover,
        },
    },
}

with open(os.path.join(out, "summary.json"), "w", encoding="utf-8") as f:
    json.dump(summary, f, ensure_ascii=False, indent=2)

print(json.dumps(summary, ensure_ascii=False, indent=2))
PY

echo "[8/8] done: $OUT_DIR/summary.json"
