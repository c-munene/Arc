#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
tmpdir=tmp/tls_smoke
mkdir -p "$tmpdir"
openssl req -x509 -newkey rsa:2048 -keyout "$tmpdir/key.pem" -out "$tmpdir/cert.pem" -days 1 -nodes -subj "/CN=localhost" >/dev/null 2>&1
cert_p="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/$tmpdir/cert.pem"
key_p="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/$tmpdir/key.pem"
cat > "$tmpdir/arc_tls_smoke.json" <<JSON
{
  "listen": "127.0.0.1:18443",
  "admin_listen": "127.0.0.1:19990",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {
    "entries": 256,
    "accept_multishot": false,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "buffers": {
    "buf_size": 8192,
    "buf_count": 64
  },
  "timeouts_ms": {
    "cli_read": 30000,
    "up_conn": 3000,
    "up_write": 30000,
    "up_read": 30000,
    "cli_write": 30000
  },
  "upstreams": [
    {
      "name": "u",
      "addr": "127.0.0.1:19090",
      "keepalive": 64,
      "idle_ttl_ms": 30000
    }
  ],
  "plugins": [],
  "routes": [
    {
      "path": "/",
      "upstream": "u",
      "plugins": [],
      "rate_limit": null
    }
  ],
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [
      {
        "sni": "localhost",
        "cert_pem": "$cert_p",
        "key_pem": "$key_p"
      }
    ],
    "sni_routes": []
  }
}
JSON
python3 -m http.server 19090 --bind 127.0.0.1 > "$tmpdir/backend.log" 2>&1 & BPID=$!
./target/debug/arc-gateway --config "$tmpdir/arc_tls_smoke.json" > "$tmpdir/gateway.log" 2>&1 & GPID=$!
sleep 2
H1=$(curl -sk --http1.1 https://localhost:18443/ -o /dev/null -w "code=%{http_code} total=%{time_total}")
H2=$(curl -sk --http2 https://localhost:18443/ -o /dev/null -w "code=%{http_code} total=%{time_total}")
PROTO=$(curl -sk --http2 -I https://localhost:18443/ | head -n 1)
kill $GPID $BPID >/dev/null 2>&1 || true
wait $GPID >/dev/null 2>&1 || true
wait $BPID >/dev/null 2>&1 || true
echo "$H1"
echo "$H2"
echo "$PROTO"
