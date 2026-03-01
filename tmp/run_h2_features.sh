#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

cleanup_ports() {
  for port in 18448 19998 19096 19097; do
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
}

cleanup_ports
mkdir -p tmp/h2_feature/a/a tmp/h2_feature/b/b
printf 'BACKEND_A\n' > tmp/h2_feature/a/a/index.txt
printf 'BACKEND_B\n' > tmp/h2_feature/b/b/index.txt
printf 'LIMIT_OK\n' > tmp/h2_feature/a/limited

cat > tmp/h2_feature/arc_h2_features.json <<'JSON'
{
  "listen": "127.0.0.1:18448",
  "admin_listen": "127.0.0.1:19998",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {"entries": 256, "accept_multishot": false, "tick_ms": 10, "sqpoll": false, "sqpoll_idle_ms": 0, "iopoll": false},
  "buffers": {"buf_size": 8192, "buf_count": 128},
  "timeouts_ms": {"cli_handshake": 1000, "cli_read": 30000, "up_conn": 3000, "up_handshake": 1000, "up_write": 30000, "up_read": 30000, "cli_write": 30000},
  "require_upstream_mtls": false,
  "upstreams": [
    {"name": "u1", "addr": "127.0.0.1:19096", "keepalive": 64, "idle_ttl_ms": 30000},
    {"name": "u2", "addr": "127.0.0.1:19097", "keepalive": 64, "idle_ttl_ms": 30000}
  ],
  "plugins": [
    {"name": "deny", "path": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/deny_plugin.wasm", "pool": 2, "timeout_ms": 5}
  ],
  "routes": [
    {"path": "/a/*", "upstream": "u1", "plugins": [], "rate_limit": null},
    {"path": "/b/*", "upstream": "u2", "plugins": [], "rate_limit": null},
    {"path": "/limited", "upstream": "u1", "plugins": [], "rate_limit": {"rps": 1, "burst": 1}},
    {"path": "/deny", "upstream": "u1", "plugins": ["deny"], "rate_limit": null}
  ],
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [
      {
        "sni": "localhost",
        "cert_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/cert.pem",
        "key_pem": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/tls_state_check/key.pem"
      }
    ],
    "sni_routes": []
  }
}
JSON

python3 -m http.server 19096 --bind 127.0.0.1 --directory tmp/h2_feature/a >/tmp/h2_feature_backend_a.log 2>&1 &
BPIDA=$!
python3 -m http.server 19097 --bind 127.0.0.1 --directory tmp/h2_feature/b >/tmp/h2_feature_backend_b.log 2>&1 &
BPIDB=$!
./target/release/arc-gateway --config tmp/h2_feature/arc_h2_features.json >/tmp/h2_feature_gateway.log 2>&1 &
GPID=$!

cleanup() {
  kill "$GPID" "$BPIDA" "$BPIDB" >/dev/null 2>&1 || true
  wait "$GPID" "$BPIDA" "$BPIDB" 2>/dev/null || true
}
trap cleanup EXIT

sleep 1.0

route_a_status="$(curl -sk --http2 --connect-timeout 3 --max-time 8 https://127.0.0.1:18448/a/index.txt -o /tmp/h2_route_a_body.txt -w '%{http_code}')"
route_b_status="$(curl -sk --http2 --connect-timeout 3 --max-time 8 https://127.0.0.1:18448/b/index.txt -o /tmp/h2_route_b_body.txt -w '%{http_code}')"
deny_status="$(curl -sk --http2 --connect-timeout 3 --max-time 8 https://127.0.0.1:18448/deny -o /tmp/h2_deny_body.txt -w '%{http_code}')"
limit_1_status="$(curl -sk --http2 --connect-timeout 3 --max-time 8 https://127.0.0.1:18448/limited -o /tmp/h2_limit_1_body.txt -w '%{http_code}')"
limit_2_status="$(curl -sk --http2 --connect-timeout 3 --max-time 8 https://127.0.0.1:18448/limited -o /tmp/h2_limit_2_body.txt -w '%{http_code}')"

route_a_body="$(tr -d '\r' </tmp/h2_route_a_body.txt | tr -d '\n')"
route_b_body="$(tr -d '\r' </tmp/h2_route_b_body.txt | tr -d '\n')"
limit_1_body="$(tr -d '\r' </tmp/h2_limit_1_body.txt | tr -d '\n')"

echo "route_a_status=${route_a_status} body=${route_a_body}"
echo "route_b_status=${route_b_status} body=${route_b_body}"
echo "deny_status=${deny_status}"
echo "limit_1_status=${limit_1_status} body=${limit_1_body}"
echo "limit_2_status=${limit_2_status}"

fail=0
if [[ "${route_a_status}" != "200" || "${route_a_body}" != "BACKEND_A" ]]; then
  echo "FAIL: route /a/*"
  fail=1
fi
if [[ "${route_b_status}" != "200" || "${route_b_body}" != "BACKEND_B" ]]; then
  echo "FAIL: route /b/*"
  fail=1
fi
if [[ "${deny_status}" != "451" ]]; then
  echo "FAIL: plugin deny"
  fail=1
fi
if [[ "${limit_1_status}" != "200" || "${limit_1_body}" != "LIMIT_OK" ]]; then
  echo "FAIL: limit first request"
  fail=1
fi
if [[ "${limit_2_status}" != "429" ]]; then
  echo "FAIL: limit second request should be 429"
  fail=1
fi

if [[ "${fail}" -ne 0 ]]; then
  echo "=== gateway.log ==="
  sed -n '1,260p' /tmp/h2_feature_gateway.log || true
  echo "=== backend_a.log ==="
  sed -n '1,200p' /tmp/h2_feature_backend_a.log || true
  echo "=== backend_b.log ==="
  sed -n '1,200p' /tmp/h2_feature_backend_b.log || true
  exit 1
fi

echo "H2 feature validation passed"
