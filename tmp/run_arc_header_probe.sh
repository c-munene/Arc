#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

for port in 19448 19096 19997; do
  pids="$(
    ss -ltnp 2>/dev/null \
      | grep -E "[\\.:]${port}[[:space:]]" \
      | sed -n 's/.*pid=\([0-9]\+\).*/\1/p' \
      | sort -u \
      || true
  )"
  if [[ -n "${pids}" ]]; then
    # shellcheck disable=SC2086
    kill -9 ${pids} >/dev/null 2>&1 || true
  fi
done

cat > /tmp/arc_hdr_probe.json <<'JSON'
{
  "listen": "127.0.0.1:19448",
  "admin_listen": "127.0.0.1:19997",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 64},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 30000, "up_conn": 3000, "up_handshake": 1000, "up_write": 30000, "up_read": 30000, "cli_write": 30000},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19096", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}],
  "downstream_tls": {"enable_h2": true, "certificates": [{"sni": "localhost", "cert_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem", "key_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem"}], "sni_routes": []}
}
JSON

python3 -m http.server 19096 --bind 127.0.0.1 >/tmp/arc_hdr_backend.log 2>&1 &
BPID=$!
./target/release/arc-gateway --config /tmp/arc_hdr_probe.json >/tmp/arc_hdr_arc.log 2>&1 &
GPID=$!

cleanup() {
  kill "$GPID" "$BPID" >/dev/null 2>&1 || true
  wait "$GPID" "$BPID" 2>/dev/null || true
}
trap cleanup EXIT

sleep 1.0
echo "--- response headers ---"
curl -sk --http1.1 -D /tmp/arc_hdr_headers.txt https://127.0.0.1:19448/ -o /tmp/arc_hdr_body.txt
sed -n '1,120p' /tmp/arc_hdr_headers.txt
echo "--- body bytes ---"
wc -c /tmp/arc_hdr_body.txt
