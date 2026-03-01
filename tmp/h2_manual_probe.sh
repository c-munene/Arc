#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

ARC_PORT="${ARC_PORT:-19448}"
BACKEND_PORT="${BACKEND_PORT:-19096}"
ADMIN_PORT="${ADMIN_PORT:-19997}"

cleanup() {
  nginx -c /tmp/arc_h2_clean_backend.conf -p /tmp/ -s stop >/dev/null 2>&1 || true
  if [[ -n "${APID:-}" ]]; then
    kill "${APID}" >/dev/null 2>&1 || true
    wait "${APID}" 2>/dev/null || true
  fi
  for p in "${ARC_PORT}" "${BACKEND_PORT}" "${ADMIN_PORT}"; do
    pids="$(
      ss -ltnp 2>/dev/null \
        | awk -v pp=":${p}" '
            $4 ~ pp {
              if (match($0, /pid=[0-9]+/)) {
                print substr($0, RSTART + 4, RLENGTH - 4)
              }
            }' \
        | sort -u || true
    )"
    if [[ -n "${pids}" ]]; then
      # shellcheck disable=SC2086
      kill -9 ${pids} >/dev/null 2>&1 || true
    fi
  done
}
trap cleanup EXIT
cleanup

mkdir -p /tmp/arc_h2_clean_backend
python3 - <<'PY'
from pathlib import Path
Path("/tmp/arc_h2_clean_backend/index.html").write_text("x" * 4096)
PY

cat > /tmp/arc_h2_clean_backend.conf <<'NGX'
pid /tmp/arc_h2_clean_backend.pid;
events { worker_connections 4096; }
http {
    access_log off;
    sendfile on;
    keepalive_timeout 65;
    server {
        listen 19096;
        root /tmp/arc_h2_clean_backend;
        location / {
            try_files $uri /index.html;
        }
    }
}
NGX

nginx -c /tmp/arc_h2_clean_backend.conf -p /tmp/ >/tmp/arc_h2_clean_backend.log 2>&1
./target/release/arc-gateway --config /tmp/arc_h2_clean_arc.json >/tmp/arc_h2_manual_arc.log 2>&1 &
APID=$!
sleep 1

export H2LOAD_BIN=/tmp/nghttp2-client-extract/usr/bin/h2load
export LD_LIBRARY_PATH=/tmp/nghttp2-libs/usr/lib/x86_64-linux-gnu:/tmp/nghttp2-libs/lib/x86_64-linux-gnu

timeout "${H2_TIMEOUT_SEC:-180}s" \
  "${H2LOAD_BIN}" \
  -n "${H2_REQS:-10000}" \
  -c "${H2_CLIENTS:-32}" \
  -m "${H2_STREAMS:-10}" \
  -t "${H2_THREADS:-2}" \
  --sni localhost \
  "https://127.0.0.1:${ARC_PORT}/" > /tmp/arc_h2_manual_arc.txt 2>&1 || true

echo "--- H2LOAD TAIL ---"
tail -n 40 /tmp/arc_h2_manual_arc.txt || true

echo "--- METRICS ---"
curl -sS "http://127.0.0.1:${ADMIN_PORT}/metrics" \
  | grep -E '^arc_(accepted_total|accept_rejected_total|requests_total|responses_total|ring_sq_dropped_total|ring_cq_overflow_total) ' || true

echo "--- ARC LOG TAIL ---"
tail -n 80 /tmp/arc_h2_manual_arc.log || true
