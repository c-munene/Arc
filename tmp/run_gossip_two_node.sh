#!/usr/bin/env bash
set -euo pipefail

ROOT=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
TMP="$ROOT/tmp/gossip_two_node"
mkdir -p "$TMP"

A_CFG="$TMP/node_a.json"
A_CFG_V2="$TMP/node_a_v2.json"
B_CFG="$TMP/node_b.json"
BACK_PY="$TMP/backend.py"
A_LOG="$TMP/node_a.log"
B_LOG="$TMP/node_b.log"
BACK_LOG="$TMP/backend.log"
RES="$TMP/result.txt"

echo -n > "$RES"

cleanup() {
  set +e
  [[ -n "${APID:-}" ]] && kill "$APID" >/dev/null 2>&1 || true
  [[ -n "${BPID:-}" ]] && kill "$BPID" >/dev/null 2>&1 || true
  [[ -n "${SPID:-}" ]] && kill "$SPID" >/dev/null 2>&1 || true
  wait "$APID" "$BPID" "$SPID" 2>/dev/null || true
}
trap cleanup EXIT

# clear stale listeners
for port in 18601 18602 19601 19602 20101 20102 22101 22102 19110; do
  pids="$(ss -ltnp 2>/dev/null | grep -E "[\\.:]${port}[[:space:]]" | sed -n 's/.*pid=\([0-9]\+\).*/\1/p' | sort -u || true)"
  if [[ -n "$pids" ]]; then
    kill -9 $pids >/dev/null 2>&1 || true
  fi
done

cat > "$BACK_PY" <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"ok\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

HTTPServer(("127.0.0.1", 19110), H).serve_forever()
PY

python3 "$BACK_PY" >"$BACK_LOG" 2>&1 &
SPID=$!

cat > "$A_CFG" <<'JSON'
{
  "listen": "127.0.0.1:18601",
  "admin_listen": "127.0.0.1:19601",
  "listen_backlog": 2048,
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 128},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 30000, "up_conn": 3000, "up_handshake": 1000, "up_write": 30000, "up_read": 30000, "cli_write": 30000},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19110", "keepalive": 16, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "control_plane": {
    "enabled": true,
    "bind": "127.0.0.1:20101",
    "role": "leader",
    "node_id": "node-a",
    "peers": ["127.0.0.1:29999"],
    "quorum": 0,
    "auth_token": "tkn",
    "pull_from": null,
    "pull_interval_ms": 500,
    "peer_timeout_ms": 800,
    "longpoll_timeout_ms": 2000,
    "peer_concurrency": 4,
    "runtime_threads": 1,
    "compile_threads": 1,
    "max_body_bytes": 2097152
  },
  "cluster_circuit": {
    "failure_threshold": 3,
    "circuit_open_ms": 2000,
    "quorum": 1,
    "half_open_probe_interval_ms": 200
  },
  "cluster": {
    "gossip": {
      "enabled": true,
      "bind": "127.0.0.1:22101",
      "advertise": "127.0.0.1:22101",
      "peers": ["127.0.0.1:22102"],
      "interval": "100ms",
      "fanout": 3,
      "suspicion_timeout": "1000ms",
      "dead_timeout": "6000ms",
      "max_message_size": 1400,
      "retransmit_multiplier": 2
    },
    "fallback": {
      "http_push": false
    }
  }
}
JSON

cat > "$A_CFG_V2" <<'JSON'
{
  "listen": "127.0.0.1:18601",
  "admin_listen": "127.0.0.1:19601",
  "listen_backlog": 3072,
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 128},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 30000, "up_conn": 3000, "up_handshake": 1000, "up_write": 30000, "up_read": 30000, "cli_write": 30000},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19110", "keepalive": 16, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "control_plane": {
    "enabled": true,
    "bind": "127.0.0.1:20101",
    "role": "leader",
    "node_id": "node-a",
    "peers": ["127.0.0.1:29999"],
    "quorum": 0,
    "auth_token": "tkn",
    "pull_from": null,
    "pull_interval_ms": 500,
    "peer_timeout_ms": 800,
    "longpoll_timeout_ms": 2000,
    "peer_concurrency": 4,
    "runtime_threads": 1,
    "compile_threads": 1,
    "max_body_bytes": 2097152
  },
  "cluster_circuit": {
    "failure_threshold": 3,
    "circuit_open_ms": 2000,
    "quorum": 1,
    "half_open_probe_interval_ms": 200
  },
  "cluster": {
    "gossip": {
      "enabled": true,
      "bind": "127.0.0.1:22101",
      "advertise": "127.0.0.1:22101",
      "peers": ["127.0.0.1:22102"],
      "interval": "100ms",
      "fanout": 3,
      "suspicion_timeout": "1000ms",
      "dead_timeout": "6000ms",
      "max_message_size": 1400,
      "retransmit_multiplier": 2
    },
    "fallback": {
      "http_push": false
    }
  }
}
JSON

cat > "$B_CFG" <<'JSON'
{
  "listen": "127.0.0.1:18602",
  "admin_listen": "127.0.0.1:19602",
  "listen_backlog": 2048,
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 128},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 30000, "up_conn": 3000, "up_handshake": 1000, "up_write": 30000, "up_read": 30000, "cli_write": 30000},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19110", "keepalive": 16, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "control_plane": {
    "enabled": true,
    "bind": "127.0.0.1:20102",
    "role": "follower",
    "node_id": "node-b",
    "peers": [],
    "quorum": 0,
    "auth_token": "tkn",
    "pull_from": "127.0.0.1:20101",
    "pull_interval_ms": 500,
    "peer_timeout_ms": 800,
    "longpoll_timeout_ms": 2000,
    "peer_concurrency": 4,
    "runtime_threads": 1,
    "compile_threads": 1,
    "max_body_bytes": 2097152
  },
  "cluster_circuit": {
    "failure_threshold": 3,
    "circuit_open_ms": 2000,
    "quorum": 1,
    "half_open_probe_interval_ms": 200
  },
  "cluster": {
    "gossip": {
      "enabled": true,
      "bind": "127.0.0.1:22102",
      "advertise": "127.0.0.1:22102",
      "peers": ["127.0.0.1:22101"],
      "interval": "100ms",
      "fanout": 3,
      "suspicion_timeout": "1000ms",
      "dead_timeout": "6000ms",
      "max_message_size": 1400,
      "retransmit_multiplier": 2
    },
    "fallback": {
      "http_push": false
    }
  }
}
JSON

cargo build -p arc-gateway >/dev/null

./target/debug/arc-gateway --config "$A_CFG" >"$A_LOG" 2>&1 &
APID=$!
./target/debug/arc-gateway --config "$B_CFG" >"$B_LOG" 2>&1 &
BPID=$!

ready() {
  local url="$1"
  for _ in $(seq 1 120); do
    if curl -sS --noproxy "*" "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

ready "http://127.0.0.1:20101/v1/status"
ready "http://127.0.0.1:20102/v1/status"

echo "[check1] B receives A gossip membership" | tee -a "$RES"
found_alive=0
for _ in $(seq 1 80); do
  curl -sS --noproxy "*" -H "Authorization: Bearer tkn" http://127.0.0.1:20102/cluster/members > "$TMP/members_b.json" || true
  if python3 - "$TMP/members_b.json" <<'PY'
import json,sys
p=sys.argv[1]
try:
    arr=json.load(open(p,'r',encoding='utf-8'))
except Exception:
    sys.exit(1)
for x in arr:
    if x.get('id')=='node-a' and x.get('status')=='alive':
        print('alive')
        sys.exit(0)
sys.exit(1)
PY
  then
    found_alive=1
    break
  fi
  sleep 0.2
done

echo "b_sees_a_alive=$found_alive" | tee -a "$RES"
if [[ "$found_alive" != "1" ]]; then
  echo "FAIL: B did not see A alive via gossip" | tee -a "$RES"
  exit 1
fi

echo "[check2] config sync via gossip only (http_push fallback disabled)" | tee -a "$RES"
a_before=$(curl -sS --noproxy "*" http://127.0.0.1:20101/v1/status | python3 -c 'import json,sys; print(json.load(sys.stdin)["generation"])')
b_before=$(curl -sS --noproxy "*" http://127.0.0.1:20102/v1/status | python3 -c 'import json,sys; print(json.load(sys.stdin)["generation"])')

resp=$(curl -sS --noproxy "*" -H "Authorization: Bearer tkn" -H "Content-Type: application/json" --data-binary @"$A_CFG_V2" http://127.0.0.1:20101/v1/cluster/config)
echo "cluster_config_resp=$resp" | tee -a "$RES"

scope=$(python3 - "$resp" <<'PY'
import json,sys
obj=json.loads(sys.argv[1])
print(obj.get('scope',''))
PY
)
new_gen=$(python3 - "$resp" <<'PY'
import json,sys
obj=json.loads(sys.argv[1])
print(obj.get('generation',0))
PY
)

echo "scope=$scope new_gen=$new_gen a_before=$a_before b_before=$b_before" | tee -a "$RES"
if [[ "$scope" != "gossip" ]]; then
  echo "FAIL: expected scope=gossip (means no HTTP push path)" | tee -a "$RES"
  exit 1
fi

b_synced=0
for _ in $(seq 1 100); do
  b_now=$(curl -sS --noproxy "*" http://127.0.0.1:20102/v1/status | python3 -c 'import json,sys; print(json.load(sys.stdin)["generation"])')
  if [[ "$b_now" == "$new_gen" ]]; then
    b_synced=1
    break
  fi
  sleep 0.2
done

echo "b_synced_to_new_gen=$b_synced" | tee -a "$RES"
if [[ "$b_synced" != "1" ]]; then
  echo "FAIL: B generation did not sync to leader via gossip" | tee -a "$RES"
  exit 1
fi

echo "[check3] A down -> B detects offline" | tee -a "$RES"
kill "$APID" >/dev/null 2>&1 || true
wait "$APID" 2>/dev/null || true
APID=""

b_detected=0
for _ in $(seq 1 80); do
  curl -sS --noproxy "*" -H "Authorization: Bearer tkn" http://127.0.0.1:20102/cluster/members > "$TMP/members_b_after_kill.json" || true
  if python3 - "$TMP/members_b_after_kill.json" <<'PY'
import json,sys
p=sys.argv[1]
try:
    arr=json.load(open(p,'r',encoding='utf-8'))
except Exception:
    sys.exit(1)
for x in arr:
    if x.get('id')=='node-a':
        st=(x.get('status') or '').lower()
        if st!='alive':
            print(st)
            sys.exit(0)
        sys.exit(1)
# missing from list also means detected as gone after dead-time cleanup
print('missing')
sys.exit(0)
PY
  then
    b_detected=1
    break
  fi
  sleep 0.2
done

echo "b_detected_a_offline=$b_detected" | tee -a "$RES"
if [[ "$b_detected" != "1" ]]; then
  echo "FAIL: B did not detect A offline" | tee -a "$RES"
  exit 1
fi

echo "ALL_GOSSIP_CHECKS_PASSED" | tee -a "$RES"
cat "$RES"
