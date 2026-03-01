#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc"
OUT_DIR="$ROOT/tmp/cluster_circuit_probe"
ARC_A_PORT=18281
ARC_B_PORT=18282
ARC_A_ADMIN=19931
ARC_B_ADMIN=19932
UP_ADDR="127.0.0.1:19299" # intentionally down

mkdir -p "$OUT_DIR"
cd "$ROOT"

cleanup() {
  set +e
  if [ -n "${ARC_A_PID:-}" ]; then kill "$ARC_A_PID" >/dev/null 2>&1 || true; fi
  if [ -n "${ARC_B_PID:-}" ]; then kill "$ARC_B_PID" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

export CARGO_HOME=/mnt/c/Users/win11/.cargo
if ! CARGO_NET_OFFLINE=true cargo build -p arc-gateway --offline >/dev/null 2>&1; then
  cargo build -p arc-gateway >/dev/null
fi

cat > "$OUT_DIR/arc_a.json" <<JSON
{
  "listen": "127.0.0.1:$ARC_A_PORT",
  "admin_listen": "127.0.0.1:$ARC_A_ADMIN",
  "workers": 1,
  "control_plane": {
    "enabled": true,
    "bind": "127.0.0.1:29931",
    "role": "standalone",
    "node_id": "node-a",
    "peers": ["http://127.0.0.1:29932"],
    "quorum": 0,
    "auth_token": null,
    "pull_from": null,
    "pull_interval_ms": 300,
    "peer_timeout_ms": 500
  },
  "linger_ms": 300,
  "io_uring": {"entries": 1024, "accept_multishot": true, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 256},
  "timeouts_ms": {"cli_handshake": 3000, "cli_read": 5000, "up_conn": 500, "up_handshake": 3000, "up_write": 5000, "up_read": 5000, "cli_write": 5000},
  "require_upstream_mtls": false,
  "global_rate_limit": {"backend": "in_memory"},
  "upstreams": [{"name": "default", "addr": "$UP_ADDR", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "default", "plugins": [], "rate_limit": {"rps": 100000, "burst": 100000}}]
}
JSON

cat > "$OUT_DIR/arc_b.json" <<JSON
{
  "listen": "127.0.0.1:$ARC_B_PORT",
  "admin_listen": "127.0.0.1:$ARC_B_ADMIN",
  "workers": 1,
  "control_plane": {
    "enabled": true,
    "bind": "127.0.0.1:29932",
    "role": "standalone",
    "node_id": "node-b",
    "peers": ["http://127.0.0.1:29931"],
    "quorum": 0,
    "auth_token": null,
    "pull_from": null,
    "pull_interval_ms": 300,
    "peer_timeout_ms": 500
  },
  "linger_ms": 300,
  "io_uring": {"entries": 1024, "accept_multishot": true, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 256},
  "timeouts_ms": {"cli_handshake": 3000, "cli_read": 5000, "up_conn": 500, "up_handshake": 3000, "up_write": 5000, "up_read": 5000, "cli_write": 5000},
  "require_upstream_mtls": false,
  "global_rate_limit": {"backend": "in_memory"},
  "upstreams": [{"name": "default", "addr": "$UP_ADDR", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "default", "plugins": [], "rate_limit": {"rps": 100000, "burst": 100000}}]
}
JSON

http_code() {
  local url="$1"
  local code
  code=$(curl -sS -o /dev/null -w "%{http_code}" "$url" 2>/dev/null || true)
  if [ -z "$code" ]; then
    code="000"
  fi
  printf "%s" "$code"
}

./target/debug/arc-gateway --config "$OUT_DIR/arc_a.json" >"$OUT_DIR/arc_a.log" 2>&1 &
ARC_A_PID=$!
./target/debug/arc-gateway --config "$OUT_DIR/arc_b.json" >"$OUT_DIR/arc_b.log" 2>&1 &
ARC_B_PID=$!

for _ in $(seq 1 80); do
  code_a=$(http_code "http://127.0.0.1:$ARC_A_PORT/")
  code_b=$(http_code "http://127.0.0.1:$ARC_B_PORT/")
  if [ "$code_a" != "000" ] && [ "$code_b" != "000" ]; then
    break
  fi
  sleep 0.1
done

baseline_b=$(http_code "http://127.0.0.1:$ARC_B_PORT/")

: > "$OUT_DIR/a_codes.txt"
for _ in $(seq 1 12); do
  c=$(http_code "http://127.0.0.1:$ARC_A_PORT/")
  echo "$c" >> "$OUT_DIR/a_codes.txt"
  sleep 0.05
done

sleep 1.2
peer_b_1=$(http_code "http://127.0.0.1:$ARC_B_PORT/")
peer_b_2=$(http_code "http://127.0.0.1:$ARC_B_PORT/")

snap_a=$(curl -sS "http://127.0.0.1:29931/v1/cluster/circuit/local" || echo "{}")
snap_b=$(curl -sS "http://127.0.0.1:29932/v1/cluster/circuit/local" || echo "{}")

sleep 4
post_expire_b=$(http_code "http://127.0.0.1:$ARC_B_PORT/")

python3 - "$OUT_DIR" "$baseline_b" "$peer_b_1" "$peer_b_2" "$post_expire_b" "$snap_a" "$snap_b" <<'PY'
import json
import sys
from pathlib import Path

out = Path(sys.argv[1])
baseline_b = sys.argv[2]
peer_b_1 = sys.argv[3]
peer_b_2 = sys.argv[4]
post_expire_b = sys.argv[5]
snap_a_raw = sys.argv[6]
snap_b_raw = sys.argv[7]

codes = [line.strip() for line in (out / "a_codes.txt").read_text(encoding="utf-8").splitlines() if line.strip()]

def parse_json(s):
    try:
        return json.loads(s)
    except Exception:
        return {"raw": s}

summary = {
    "baseline_b": baseline_b,
    "a_trigger_codes": codes,
    "peer_b_after_a_failures": [peer_b_1, peer_b_2],
    "post_expire_b": post_expire_b,
    "snapshot_a": parse_json(snap_a_raw),
    "snapshot_b": parse_json(snap_b_raw),
}

(summary_path := out / "summary.json").write_text(
    json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8"
)
print(json.dumps(summary, ensure_ascii=False, indent=2))
print(f"summary_path={summary_path}")
PY
