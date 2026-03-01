#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

ARC_PORT="${ARC_PORT:-19448}"
NGX_PORT="${NGX_PORT:-19449}"
UP_PORT="${UP_PORT:-19096}"
ADMIN_PORT="${ADMIN_PORT:-19997}"
H2_REQS="${H2_REQS:-2000}"
H2_CLIENTS="${H2_CLIENTS:-32}"
H2_STREAMS="${H2_STREAMS:-10}"
H2_THREADS="${H2_THREADS:-2}"
CLI_HANDSHAKE_MS="${CLI_HANDSHAKE_MS:-1000}"
CLI_READ_MS="${CLI_READ_MS:-120000}"
CLI_WRITE_MS="${CLI_WRITE_MS:-120000}"
UP_CONN_MS="${UP_CONN_MS:-3000}"
UP_HANDSHAKE_MS="${UP_HANDSHAKE_MS:-1000}"
UP_WRITE_MS="${UP_WRITE_MS:-120000}"
UP_READ_MS="${UP_READ_MS:-120000}"

H2LOAD_BIN="/tmp/nghttp2-client-extract/usr/bin/h2load"
H2LIB_PATH="/tmp/nghttp2-libs/usr/lib/x86_64-linux-gnu:/tmp/nghttp2-libs/lib/x86_64-linux-gnu"

bootstrap_h2load() {
  if [[ -x "$H2LOAD_BIN" ]]; then
    return 0
  fi

  cd /tmp
  apt-get download nghttp2-client libjemalloc2 libev4t64 >/dev/null
  mkdir -p /tmp/nghttp2-client-extract /tmp/nghttp2-libs
  dpkg-deb -x /tmp/nghttp2-client_*.deb /tmp/nghttp2-client-extract
  dpkg-deb -x /tmp/libjemalloc2_*.deb /tmp/nghttp2-libs
  dpkg-deb -x /tmp/libev4t64_*.deb /tmp/nghttp2-libs
}

cleanup_ports() {
  for port in "$ARC_PORT" "$NGX_PORT" "$UP_PORT" "$ADMIN_PORT"; do
    pids="$(
      ss -ltnp 2>/dev/null \
        | awk -v p=":${port}" '
            $4 ~ p {
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

bootstrap_h2load
cleanup_ports

cat > /tmp/arc_h2load_nginx.conf <<'NGX'
pid /tmp/arc_h2load_nginx.pid;
events { worker_connections 4096; }
http {
    access_log off;
    sendfile on;
    server {
        listen 19449 ssl http2;
        ssl_certificate /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem;
        ssl_certificate_key /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem;
        ssl_protocols TLSv1.2 TLSv1.3;
        location / {
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_pass http://127.0.0.1:19096;
        }
    }
}
NGX

cat > /tmp/arc_h2load_arc.json <<'ARC'
{
  "listen": "127.0.0.1:19448",
  "admin_listen": "127.0.0.1:19997",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "accept_prepost": 32, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "listen_backlog": 4096,
  "buffers": {"buf_size": 8192, "buf_count": 64},
  "timeouts_ms": {"cli_handshake": __CLI_HANDSHAKE_MS__, "cli_read": __CLI_READ_MS__, "up_conn": __UP_CONN_MS__, "up_handshake": __UP_HANDSHAKE_MS__, "up_write": __UP_WRITE_MS__, "up_read": __UP_READ_MS__, "cli_write": __CLI_WRITE_MS__},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19096", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "downstream_tls": {"enable_h2": true, "certificates": [{"sni": "localhost", "cert_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem", "key_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem"}], "sni_routes": []}
}
ARC

sed -i "s/__CLI_HANDSHAKE_MS__/${CLI_HANDSHAKE_MS}/g" /tmp/arc_h2load_arc.json
sed -i "s/__CLI_READ_MS__/${CLI_READ_MS}/g" /tmp/arc_h2load_arc.json
sed -i "s/__UP_CONN_MS__/${UP_CONN_MS}/g" /tmp/arc_h2load_arc.json
sed -i "s/__UP_HANDSHAKE_MS__/${UP_HANDSHAKE_MS}/g" /tmp/arc_h2load_arc.json
sed -i "s/__UP_WRITE_MS__/${UP_WRITE_MS}/g" /tmp/arc_h2load_arc.json
sed -i "s/__UP_READ_MS__/${UP_READ_MS}/g" /tmp/arc_h2load_arc.json
sed -i "s/__CLI_WRITE_MS__/${CLI_WRITE_MS}/g" /tmp/arc_h2load_arc.json

python3 -m http.server "$UP_PORT" --bind 127.0.0.1 >/tmp/arc_h2load_backend.log 2>&1 &
BPID=$!
./target/release/arc-gateway --config /tmp/arc_h2load_arc.json >/tmp/arc_h2load_arc.log 2>&1 &
APID=$!
nginx -c /tmp/arc_h2load_nginx.conf -p /tmp/ >/tmp/arc_h2load_nginx.log 2>&1

cleanup() {
  nginx -c /tmp/arc_h2load_nginx.conf -p /tmp/ -s stop >/dev/null 2>&1 || true
  kill "$APID" "$BPID" >/dev/null 2>&1 || true
  wait "$APID" "$BPID" 2>/dev/null || true
  cleanup_ports
}
trap cleanup EXIT

sleep 1

run_case() {
  local name="$1"
  local uri="$2"
  local out="$3"
  LD_LIBRARY_PATH="$H2LIB_PATH" "$H2LOAD_BIN" \
    -n "${H2_REQS}" \
    -c "${H2_CLIENTS}" \
    -m "${H2_STREAMS}" \
    -t "${H2_THREADS}" \
    --sni localhost \
    "$uri" >"$out" 2>&1
  echo "=== ${name} ==="
  cat "$out"
}

run_case ARC "https://127.0.0.1:${ARC_PORT}/" "/tmp/arc_h2load_arc.txt"
run_case NGINX "https://127.0.0.1:${NGX_PORT}/" "/tmp/arc_h2load_nginx.txt"

echo "=== ARC METRICS SNAPSHOT ==="
curl -sS "http://127.0.0.1:${ADMIN_PORT}/metrics" \
  | grep -E '^arc_(accepted_total|accept_rejected_total|requests_total|responses_total|ring_sq_dropped_total|ring_cq_overflow_total) ' || true
