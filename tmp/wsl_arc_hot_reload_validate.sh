#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/wsl_hot_reload_validate"
ARC_BIN="$ROOT/target-wsl/release/arc-gateway"
NOOP_WASM="$ROOT/tmp/noop_plugin.wasm"
WRK_THREADS="${WRK_THREADS:-8}"
WRK_CONNECTIONS="${WRK_CONNECTIONS:-256}"
WRK_DURATION="${WRK_DURATION:-24s}"

mkdir -p "$WORK/backend_a_runtime/logs" "$WORK/backend_b_runtime/logs"

if [[ ! -x "$ARC_BIN" ]]; then
  echo "arc-gateway binary not found: $ARC_BIN" >&2
  exit 1
fi
if [[ ! -f "$NOOP_WASM" ]]; then
  echo "noop wasm not found: $NOOP_WASM" >&2
  exit 1
fi

cat > "$WORK/backend_a.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_hot_reload_validate/backend_a.pid;
events {
    worker_connections 8192;
    use epoll;
}
http {
    access_log off;
    error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_hot_reload_validate/backend_a_error.log warn;
    server {
        listen 127.0.0.1:3000;
        location / {
            return 200 "A\n";
        }
    }
}
NGINX

cat > "$WORK/backend_b.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_hot_reload_validate/backend_b.pid;
events {
    worker_connections 8192;
    use epoll;
}
http {
    access_log off;
    error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_hot_reload_validate/backend_b_error.log warn;
    server {
        listen 127.0.0.1:3001;
        location / {
            return 200 "B\n";
        }
    }
}
NGINX

cat > "$WORK/arc_a.json" <<JSON
{
  "listen": "127.0.0.1:8080",
  "admin_listen": "127.0.0.1:9900",
  "workers": 4,
  "linger_ms": 300,
  "io_uring": {
    "entries": 4096,
    "accept_multishot": true,
    "tick_ms": 10,
    "sqpoll": true,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "buffers": {
    "buf_size": 8192,
    "buf_count": 1024
  },
  "timeouts_ms": {
    "cli_read": 30000,
    "up_conn": 3000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "upstreams": [
    {"name": "app_a", "addr": "127.0.0.1:3000", "keepalive": 1024, "idle_ttl_ms": 30000},
    {"name": "app_b", "addr": "127.0.0.1:3001", "keepalive": 1024, "idle_ttl_ms": 30000}
  ],
  "plugins": [],
  "routes": [
    {
      "path": "/*",
      "upstream": "app_a",
      "plugins": [],
      "rate_limit": null
    }
  ]
}
JSON

cat > "$WORK/arc_b.json" <<JSON
{
  "listen": "127.0.0.1:8080",
  "admin_listen": "127.0.0.1:9900",
  "workers": 4,
  "linger_ms": 300,
  "io_uring": {
    "entries": 4096,
    "accept_multishot": true,
    "tick_ms": 10,
    "sqpoll": true,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "buffers": {
    "buf_size": 8192,
    "buf_count": 1024
  },
  "timeouts_ms": {
    "cli_read": 30000,
    "up_conn": 3000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "upstreams": [
    {"name": "app_a", "addr": "127.0.0.1:3000", "keepalive": 1024, "idle_ttl_ms": 30000},
    {"name": "app_b", "addr": "127.0.0.1:3001", "keepalive": 1024, "idle_ttl_ms": 30000}
  ],
  "plugins": [],
  "routes": [
    {
      "path": "/*",
      "upstream": "app_b",
      "plugins": [],
      "rate_limit": null
    }
  ]
}
JSON

cp "$WORK/arc_a.json" "$WORK/arc.json"

ARC_PID=""

cleanup() {
  set +e
  if [[ -n "$ARC_PID" ]]; then kill "$ARC_PID" >/dev/null 2>&1 || true; fi
  nginx -p "$WORK/backend_a_runtime" -c "$WORK/backend_a.conf" -s stop >/dev/null 2>&1 || true
  nginx -p "$WORK/backend_b_runtime" -c "$WORK/backend_b.conf" -s stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

pkill -f "arc-gateway --config $WORK/arc.json" >/dev/null 2>&1 || true
nginx -p "$WORK/backend_a_runtime" -c "$WORK/backend_a.conf" -s stop >/dev/null 2>&1 || true
nginx -p "$WORK/backend_b_runtime" -c "$WORK/backend_b.conf" -s stop >/dev/null 2>&1 || true

nginx -p "$WORK/backend_a_runtime" -c "$WORK/backend_a.conf"
nginx -p "$WORK/backend_b_runtime" -c "$WORK/backend_b.conf"

"$ARC_BIN" --config "$WORK/arc.json" > "$WORK/arc.log" 2>&1 &
ARC_PID=$!

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:8080/x" >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done

PRE_BODY="$(curl -fsS "http://127.0.0.1:8080/x" 2>/dev/null || true)"
echo "$PRE_BODY" > "$WORK/pre_body.txt"

wrk -t"$WRK_THREADS" -c"$WRK_CONNECTIONS" -d"$WRK_DURATION" --latency "http://127.0.0.1:8080/x" \
  | tee "$WORK/wrk_reload.txt" >/dev/null &
WRK_PID=$!
echo "$WRK_THREADS,$WRK_CONNECTIONS,$WRK_DURATION" > "$WORK/wrk_meta.txt"

sleep 8
cp "$WORK/arc_b.json" "$WORK/arc.json.next"
mv -f "$WORK/arc.json.next" "$WORK/arc.json"
date +%s%3N > "$WORK/reload_epoch_ms.txt"

SWITCH_SEEN=0
for _ in $(seq 1 50); do
  body="$(curl -fsS "http://127.0.0.1:8080/x" 2>/dev/null || true)"
  if [[ "$body" == "B" ]]; then
    SWITCH_SEEN=1
    break
  fi
  sleep 0.2
done
echo "$SWITCH_SEEN" > "$WORK/switch_seen_during_load.txt"

wait "$WRK_PID"

POST_BODY="$(curl -fsS "http://127.0.0.1:8080/x" 2>/dev/null || true)"
echo "$POST_BODY" > "$WORK/post_body.txt"

python3 - <<'PY'
import json
import re
from pathlib import Path

work = Path("/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_hot_reload_validate")
txt = (work / "wrk_reload.txt").read_text(encoding="utf-8", errors="ignore")

lat = re.search(r"Latency\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)", txt)
sock = re.search(r"Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)", txt)
m_non = re.search(r"Non-2xx or 3xx responses:\s+(\d+)", txt)
m_req = re.search(r"([0-9]+)\s+requests in", txt)
m_rps = re.search(r"Requests/sec:\s+([0-9.]+)", txt)

def to_ms(v, unit):
    u = unit.lower()
    if u == "us":
        return v / 1000.0
    if u == "ms":
        return v
    if u == "s":
        return v * 1000.0
    return v

lat_avg = to_ms(float(lat.group(1)), lat.group(2)) if lat else 0.0
lat_stdev = to_ms(float(lat.group(3)), lat.group(4)) if lat else 0.0
lat_max = to_ms(float(lat.group(5)), lat.group(6)) if lat else 0.0
socket_errors = sum(int(x) for x in sock.groups()) if sock else 0
non2xx = int(m_non.group(1)) if m_non else 0
total_requests = int(m_req.group(1)) if m_req else 0
rps = float(m_rps.group(1)) if m_rps else 0.0

pre_body = (work / "pre_body.txt").read_text(encoding="utf-8", errors="ignore").strip()
post_body = (work / "post_body.txt").read_text(encoding="utf-8", errors="ignore").strip()
switch_seen = (work / "switch_seen_during_load.txt").read_text(encoding="utf-8", errors="ignore").strip() == "1"
wrk_threads, wrk_connections, wrk_duration = (work / "wrk_meta.txt").read_text(encoding="utf-8", errors="ignore").strip().split(",", 2)

result = {
    "wrk": {
        "threads": int(wrk_threads),
        "connections": int(wrk_connections),
        "duration": wrk_duration,
    },
    "requests_sec": rps,
    "latency_ms": {"avg": lat_avg, "stdev": lat_stdev, "max": lat_max},
    "socket_errors": socket_errors,
    "non_2xx_or_3xx": non2xx,
    "total_requests": total_requests,
    "switch_probe": {
        "before_reload_body": pre_body,
        "switch_seen_during_load": switch_seen,
        "after_reload_body": post_body,
    },
    "pass": {
        "reload_switch_effective": (pre_body == "A" and switch_seen and post_body == "B"),
        "no_socket_or_http_errors": (socket_errors == 0 and non2xx == 0),
    },
}
result["pass"]["overall"] = result["pass"]["reload_switch_effective"] and result["pass"]["no_socket_or_http_errors"]

(work / "summary.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
print(json.dumps(result, ensure_ascii=False))
PY

echo "HOT_RELOAD_SUMMARY_JSON=$WORK/summary.json"
