#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
cat > tmp/no_tls_smoke.json <<JSON
{
  "listen": "127.0.0.1:18447",
  "admin_listen": "127.0.0.1:19994",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 64},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 30000, "up_conn": 3000, "up_handshake": 1000, "up_write": 30000, "up_read": 30000, "cli_write": 30000},
  "require_upstream_mtls": false,
  "upstreams": [{"name": "u", "addr": "127.0.0.1:19095", "keepalive": 64, "idle_ttl_ms": 30000}],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}]
}
JSON
python3 -m http.server 19095 --bind 127.0.0.1 > tmp/no_tls_backend.log 2>&1 &
B=$!
./target/debug/arc-gateway --config tmp/no_tls_smoke.json > tmp/no_tls_gateway.log 2>&1 &
G=$!
sleep 2
R=$(curl -s http://127.0.0.1:18447/ -o /dev/null -w "code=%{http_code}")
kill $G $B >/dev/null 2>&1 || true
wait $G >/dev/null 2>&1 || true
wait $B >/dev/null 2>&1 || true
echo "$R"
