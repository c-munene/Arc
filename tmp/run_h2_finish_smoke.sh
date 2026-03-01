#!/usr/bin/env bash
set -euo pipefail

cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

mkdir -p tmp/h2_finish_smoke
CFG=tmp/h2_finish_smoke/arc_h2_finish.json
LOG=tmp/h2_finish_smoke/gateway.log

for port in 18459 19959; do
  pids="$(ss -ltnp 2>/dev/null | grep -E "[\\.:]${port}[[:space:]]" | sed -n "s/.*pid=\([0-9]\+\).*/\1/p" | sort -u || true)"
  if [[ -n "$pids" ]]; then
    kill -9 $pids >/dev/null 2>&1 || true
  fi
done

cat > "$CFG" <<JSON
{
  "listen": "127.0.0.1:18459",
  "admin_listen": "127.0.0.1:19959",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {
    "entries": 256,
    "accept_multishot": false,
    "accept_prepost": 8,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "buffers": {
    "buf_size": 8192,
    "buf_count": 256
  },
  "timeouts_ms": {
    "cli_handshake": 3000,
    "cli_read": 30000,
    "up_conn": 3000,
    "up_handshake": 3000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "require_upstream_mtls": false,
  "upstreams": [],
  "plugins": [],
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
  },
  "routes": [
    {
      "path": "/sni",
      "priority": 100,
      "matchers": [
        { "type": "h2", "enabled": true },
        { "type": "sni", "hosts": ["sni-hit.local"] }
      ],
      "action": {
        "type": "respond",
        "status": 200,
        "body": "sni-hit",
        "headers": [
          { "name": "x-sni-hit", "value": "1" }
        ]
      }
    },
    {
      "path": "/sni",
      "priority": 10,
      "matchers": [
        { "type": "h2", "enabled": true }
      ],
      "action": {
        "type": "respond",
        "status": 200,
        "body": "fallback",
        "headers": [
          { "name": "x-sni-hit", "value": "0" }
        ]
      }
    },
    {
      "path": "/hdr",
      "priority": 100,
      "matchers": [
        { "type": "h2", "enabled": true }
      ],
      "action": {
        "type": "respond",
        "status": 209,
        "body": "hdr-ok",
        "headers": [
          { "name": "x-h2-respond", "value": "works" },
          { "name": "cache-control", "value": "no-store" }
        ]
      }
    }
  ]
}
JSON

cargo build -p arc-gateway >/dev/null
./target/debug/arc-gateway --config "$CFG" >"$LOG" 2>&1 &
GPID=$!
cleanup() {
  kill "$GPID" >/dev/null 2>&1 || true
  wait "$GPID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

ready=0
for _ in $(seq 1 80); do
  if curl -sk --http2 --noproxy "*" --connect-timeout 1 --max-time 2 https://localhost:18459/hdr >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
if [[ "$ready" != "1" ]]; then
  echo "ready=0"
  sed -n "1,180p" "$LOG" || true
  exit 1
fi

curl -sk --http2 --noproxy "*" --resolve sni-hit.local:18459:127.0.0.1 https://sni-hit.local:18459/sni \
  -D tmp/h2_finish_smoke/sni_hit.hdr -o tmp/h2_finish_smoke/sni_hit.body -w "sni_hit=%{http_code} v=%{http_version}\n"
curl -sk --http2 --noproxy "*" https://localhost:18459/sni \
  -D tmp/h2_finish_smoke/sni_fallback.hdr -o tmp/h2_finish_smoke/sni_fallback.body -w "sni_fallback=%{http_code} v=%{http_version}\n"
curl -sk --http2 --noproxy "*" https://localhost:18459/hdr \
  -D tmp/h2_finish_smoke/hdr_resp.hdr -o tmp/h2_finish_smoke/hdr_resp.body -w "hdr_resp=%{http_code} v=%{http_version}\n"

echo -n "sni_hit_body="; tr -d "\r\n" < tmp/h2_finish_smoke/sni_hit.body; echo
echo -n "sni_fallback_body="; tr -d "\r\n" < tmp/h2_finish_smoke/sni_fallback.body; echo
echo -n "hdr_resp_body="; tr -d "\r\n" < tmp/h2_finish_smoke/hdr_resp.body; echo

grep -i "^x-sni-hit: 1" tmp/h2_finish_smoke/sni_hit.hdr >/dev/null
grep -i "^x-sni-hit: 0" tmp/h2_finish_smoke/sni_fallback.hdr >/dev/null
grep -i "^x-h2-respond: works" tmp/h2_finish_smoke/hdr_resp.hdr >/dev/null
grep -i "^cache-control: no-store" tmp/h2_finish_smoke/hdr_resp.hdr >/dev/null
echo "h2_finish_smoke=pass"
