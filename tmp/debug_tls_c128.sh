#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

ARC_PORT="${ARC_PORT:-21448}"
ADMIN_PORT="${ADMIN_PORT:-21997}"
UP_PORT="${UP_PORT:-21096}"
BUF_COUNT="${BUF_COUNT:-64}"
WORKERS="${WORKERS:-1}"
RING_ENTRIES="${RING_ENTRIES:-256}"
CONN="${CONN:-128}"
THREADS="${THREADS:-4}"
DURATION="${DURATION:-8s}"

cleanup() {
  for p in "$ARC_PORT" "$ADMIN_PORT" "$UP_PORT"; do
    local pids
    pids="$(
      ss -ltnp 2>/dev/null \
        | awk -v port=":${p}" '
            $4 ~ port {
              if (match($0, /pid=[0-9]+/)) {
                print substr($0, RSTART + 4, RLENGTH - 4)
              }
            }
          ' \
        | sort -u || true
    )"
    if [[ -n "$pids" ]]; then
      # shellcheck disable=SC2086
      kill -9 $pids >/dev/null 2>&1 || true
    fi
  done
}

cleanup

cat > /tmp/arc_debug_tls_c128.json <<EOF
{
  "listen": "127.0.0.1:${ARC_PORT}",
  "admin_listen": "127.0.0.1:${ADMIN_PORT}",
  "workers": ${WORKERS},
  "linger_ms": 100,
  "io_uring": {
    "entries": ${RING_ENTRIES},
    "accept_multishot": false,
    "accept_prepost": 32,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "listen_backlog": 4096,
  "buffers": {"buf_size": 8192, "buf_count": ${BUF_COUNT}},
  "timeouts_ms": {
    "cli_handshake": 1000,
    "cli_read": 30000,
    "up_conn": 3000,
    "up_handshake": 1000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:${UP_PORT}", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [{
      "sni": "localhost",
      "cert_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem",
      "key_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem"
    }],
    "sni_routes": []
  }
}
EOF

python3 -m http.server "${UP_PORT}" --bind 127.0.0.1 >/tmp/arc_debug_tls_backend.log 2>&1 &
BPID=$!
./target/release/arc-gateway --config /tmp/arc_debug_tls_c128.json >/tmp/arc_debug_tls_arc.log 2>&1 &
APID=$!

trap 'kill "$APID" "$BPID" >/dev/null 2>&1 || true; wait "$APID" "$BPID" 2>/dev/null || true; cleanup' EXIT

sleep 1

max_est=0
max_syn=0
sample_conn_states() {
  local est syn
  est="$(
    ss -ant \
      | awk -v port="127.0.0.1:${ARC_PORT}" '$4 == port && $1 == "ESTAB" { c++ } END { print c + 0 }'
  )"
  syn="$(
    ss -ant \
      | awk -v port="127.0.0.1:${ARC_PORT}" '$4 == port && $1 == "SYN-RECV" { c++ } END { print c + 0 }'
  )"
  if (( est > max_est )); then
    max_est="$est"
  fi
  if (( syn > max_syn )); then
    max_syn="$syn"
  fi
}

wrk -t"${THREADS}" -c"${CONN}" -d"${DURATION}" --latency "https://127.0.0.1:${ARC_PORT}/" \
  >/tmp/arc_debug_tls_wrk.txt 2>&1 &
WRKPID=$!

while kill -0 "${WRKPID}" 2>/dev/null; do
  sample_conn_states
  sleep 0.1
done
wait "${WRKPID}" || true

metrics="$(curl -sS --max-time 2 "http://127.0.0.1:${ADMIN_PORT}/metrics" || true)"

echo "=== WRK ==="
cat /tmp/arc_debug_tls_wrk.txt
echo "=== CONN_PEAK ==="
echo "max_established=${max_est}"
echo "max_syn_recv=${max_syn}"
echo "=== METRICS ==="
printf "%s\n" "$metrics" | grep -E '^arc_(accepted_total|accept_rejected_total|active_current|closed_total|requests_total|responses_total|ring_sq_dropped_total|ring_cq_overflow_total) ' || true
