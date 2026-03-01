#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
WORK="$ROOT/tmp/wsl_arc_quad_full_256_512"
ARC_BIN="$ROOT/target-wsl/release/arc-gateway"
PINGORA_BIN="$ROOT/target-wsl/release/pingora-min"
NOOP_WASM="$ROOT/tmp/noop_plugin.wasm"

WRK_THREADS="${WRK_THREADS:-8}"
WRK_DURATION="${WRK_DURATION:-15s}"
ARC_WORKERS="${ARC_WORKERS:-4}"
PINGORA_MIN_WORKERS="${PINGORA_MIN_WORKERS:-4}"

mkdir -p "$WORK/backend_runtime/logs" "$WORK/proxy_runtime/logs"

if [[ ! -x "$ARC_BIN" ]]; then
  echo "arc-gateway binary not found: $ARC_BIN" >&2
  exit 1
fi
if [[ ! -x "$PINGORA_BIN" ]]; then
  echo "pingora-min binary not found: $PINGORA_BIN" >&2
  exit 1
fi
if [[ ! -f "$NOOP_WASM" ]]; then
  echo "noop wasm not found: $NOOP_WASM" >&2
  exit 1
fi

cat > "$WORK/backend_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_quad_full_256_512/backend.pid;
events {
    worker_connections 8192;
    use epoll;
}
http {
    access_log off;
    error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_quad_full_256_512/backend_nginx_error.log warn;
    server {
        listen 127.0.0.1:3000;
        location / {
            return 200 "ok\n";
        }
    }
}
NGINX

cat > "$WORK/proxy_nginx.conf" <<'NGINX'
worker_processes auto;
pid /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_quad_full_256_512/proxy.pid;
events {
    worker_connections 8192;
    use epoll;
}
http {
    access_log off;
    error_log /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_quad_full_256_512/proxy_nginx_error.log warn;
    upstream app_backend {
        server 127.0.0.1:3000;
        keepalive 256;
    }
    server {
        listen 127.0.0.1:8081;
        location / {
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_set_header Host $host;
            proxy_pass http://app_backend;
        }
    }
}
NGINX

cat > "$WORK/arc.json" <<JSON
{
  "listen": "127.0.0.1:8080",
  "admin_listen": "127.0.0.1:9900",
  "workers": $ARC_WORKERS,
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
    {"name": "default", "addr": "127.0.0.1:3000", "keepalive": 1024, "idle_ttl_ms": 30000}
  ],
  "plugins": [
    {"name": "noop", "path": "$NOOP_WASM", "pool": 1, "timeout_ms": 2}
  ],
  "routes": [
    {
      "path": "/*",
      "upstream": "default",
      "plugins": ["noop"],
      "rate_limit": {"rps": 1000000, "burst": 1000000}
    }
  ]
}
JSON

ARC_PID=""
PINGORA_PID=""

cleanup() {
  set +e
  if [[ -n "$ARC_PID" ]]; then kill "$ARC_PID" >/dev/null 2>&1 || true; fi
  if [[ -n "$PINGORA_PID" ]]; then kill "$PINGORA_PID" >/dev/null 2>&1 || true; fi
  nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
  nginx -p "$WORK/proxy_runtime" -c "$WORK/proxy_nginx.conf" -s stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

pkill -f "arc-gateway --config $WORK/arc.json" >/dev/null 2>&1 || true
pkill -f "PINGORA_MIN_BIND=127.0.0.1:8082" >/dev/null 2>&1 || true
pkill -f "pingora-min" >/dev/null 2>&1 || true
nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf" -s stop >/dev/null 2>&1 || true
nginx -p "$WORK/proxy_runtime" -c "$WORK/proxy_nginx.conf" -s stop >/dev/null 2>&1 || true

nginx -p "$WORK/backend_runtime" -c "$WORK/backend_nginx.conf"
"$ARC_BIN" --config "$WORK/arc.json" > "$WORK/arc.log" 2>&1 &
ARC_PID=$!

PINGORA_MIN_WORKERS="$PINGORA_MIN_WORKERS" PINGORA_MIN_BIND="127.0.0.1:8082" PINGORA_MIN_UPSTREAM="127.0.0.1:3000" \
  "$PINGORA_BIN" > "$WORK/pingora_min.log" 2>&1 &
PINGORA_PID=$!

nginx -p "$WORK/proxy_runtime" -c "$WORK/proxy_nginx.conf"

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:3000/x" >/dev/null 2>&1 \
     && curl -fsS "http://127.0.0.1:8080/x" >/dev/null 2>&1 \
     && curl -fsS "http://127.0.0.1:8081/x" >/dev/null 2>&1 \
     && curl -fsS "http://127.0.0.1:8082/x" >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done

run_wrk() {
  local name="$1"
  local url="$2"
  local c="$3"
  for i in 1 2 3; do
    wrk -t"$WRK_THREADS" -c"$c" -d"$WRK_DURATION" --latency "$url" | tee "$WORK/${name}_c${c}_run${i}.txt" >/dev/null
  done
}

for c in 256 512; do
  run_wrk "backend" "http://127.0.0.1:3000/x" "$c"
  run_wrk "arc" "http://127.0.0.1:8080/x" "$c"
  run_wrk "nginx" "http://127.0.0.1:8081/x" "$c"
  run_wrk "pingora_min" "http://127.0.0.1:8082/x" "$c"
done

python3 - <<'PY'
import json
import re
from pathlib import Path

work = Path("/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/wsl_arc_quad_full_256_512")
targets = ["backend", "arc", "nginx", "pingora_min"]
conns = [256, 512]

pat_rps = re.compile(r"Requests/sec:\s+([0-9.]+)")
pat_lat = re.compile(r"Latency\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)\s+([0-9.]+)([a-zA-Z]+)")
pat_err = re.compile(r"Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)")
pat_non2xx = re.compile(r"Non-2xx or 3xx responses:\s+(\d+)")

def to_ms(v, unit):
    u = unit.lower()
    if u == "us":
        return v / 1000.0
    if u == "ms":
        return v
    if u == "s":
        return v * 1000.0
    return v

def parse_one(path: Path):
    text = path.read_text(encoding="utf-8", errors="ignore")
    m_rps = pat_rps.search(text)
    m_lat = pat_lat.search(text)
    m_err = pat_err.search(text)
    m_non = pat_non2xx.search(text)
    row = {
        "rps": float(m_rps.group(1)) if m_rps else 0.0,
        "lat_avg_ms": 0.0,
        "lat_stdev_ms": 0.0,
        "lat_max_ms": 0.0,
        "socket_errors": 0,
        "non_2xx_or_3xx": int(m_non.group(1)) if m_non else 0,
    }
    if m_lat:
        row["lat_avg_ms"] = to_ms(float(m_lat.group(1)), m_lat.group(2))
        row["lat_stdev_ms"] = to_ms(float(m_lat.group(3)), m_lat.group(4))
        row["lat_max_ms"] = to_ms(float(m_lat.group(5)), m_lat.group(6))
    if m_err:
        row["socket_errors"] = sum(int(x) for x in m_err.groups())
    return row

def agg(rows):
    n = len(rows)
    return {
        "rps": sum(r["rps"] for r in rows) / n,
        "lat_avg_ms": sum(r["lat_avg_ms"] for r in rows) / n,
        "lat_stdev_ms": sum(r["lat_stdev_ms"] for r in rows) / n,
        "lat_max_ms": sum(r["lat_max_ms"] for r in rows) / n,
        "socket_errors": sum(r["socket_errors"] for r in rows),
        "non_2xx_or_3xx": sum(r["non_2xx_or_3xx"] for r in rows),
    }

out = {"meta": {"connections": conns, "runs_per_target": 3}, "by_conn": {}}

for c in conns:
    bucket = {}
    for t in targets:
        rows = [parse_one(work / f"{t}_c{c}_run{i}.txt") for i in (1, 2, 3)]
        bucket[t] = {"runs": rows, "avg": agg(rows)}

    b = bucket["backend"]["avg"]
    a = bucket["arc"]["avg"]
    n = bucket["nginx"]["avg"]
    p = bucket["pingora_min"]["avg"]

    ranking = sorted(
        [
            {"target": "backend", "rps": b["rps"]},
            {"target": "arc", "rps": a["rps"]},
            {"target": "nginx", "rps": n["rps"]},
            {"target": "pingora_min", "rps": p["rps"]},
        ],
        key=lambda x: x["rps"],
        reverse=True,
    )

    bucket["compare"] = {
        "arc_vs_nginx_rps_ratio": a["rps"] / n["rps"] if n["rps"] else 0.0,
        "arc_vs_pingora_min_rps_ratio": a["rps"] / p["rps"] if p["rps"] else 0.0,
        "arc_vs_backend_rps_ratio": a["rps"] / b["rps"] if b["rps"] else 0.0,
        "nginx_vs_backend_rps_ratio": n["rps"] / b["rps"] if b["rps"] else 0.0,
        "pingora_min_vs_backend_rps_ratio": p["rps"] / b["rps"] if b["rps"] else 0.0,
        "latency_added_ms": {
            "arc": a["lat_avg_ms"] - b["lat_avg_ms"],
            "nginx": n["lat_avg_ms"] - b["lat_avg_ms"],
            "pingora_min": p["lat_avg_ms"] - b["lat_avg_ms"],
        },
        "rps_ranking": ranking,
    }

    out["by_conn"][str(c)] = bucket

(work / "summary.json").write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")
print(json.dumps(out, ensure_ascii=False))
PY

echo "QUAD_SUMMARY_JSON=$WORK/summary.json"
