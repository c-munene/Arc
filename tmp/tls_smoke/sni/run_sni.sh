#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
tmpdir=tmp/tls_smoke/sni
mkdir -p "$tmpdir/a" "$tmpdir/b"
echo A > "$tmpdir/a/index.html"
echo B > "$tmpdir/b/index.html"
openssl req -x509 -newkey rsa:2048 -keyout "$tmpdir/key.pem" -out "$tmpdir/cert.pem" -days 1 -nodes -subj "/CN=*.local" >/dev/null 2>&1
cert_p="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/$tmpdir/cert.pem"
key_p="/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/$tmpdir/key.pem"
cat > "$tmpdir/arc_tls_sni.json" <<JSON
{
  "listen": "127.0.0.1:18445",
  "admin_listen": "127.0.0.1:19992",
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
    { "name": "u1", "addr": "127.0.0.1:19092", "keepalive": 64, "idle_ttl_ms": 30000 },
    { "name": "u2", "addr": "127.0.0.1:19093", "keepalive": 64, "idle_ttl_ms": 30000 }
  ],
  "plugins": [],
  "routes": [
    { "path": "/", "upstream": "u1", "plugins": [], "rate_limit": null },
    { "path": "/sni/*", "upstream": "u2", "plugins": [], "rate_limit": null }
  ],
  "downstream_tls": {
    "enable_h2": true,
    "certificates": [
      { "sni": "*.local", "cert_pem": "$cert_p", "key_pem": "$key_p" }
    ],
    "sni_routes": [
      { "sni": "api.local", "path": "/sni/*" }
    ]
  }
}
JSON
python3 -m http.server 19092 --bind 127.0.0.1 -d "$tmpdir/a" > "$tmpdir/backend_a.log" 2>&1 & A=$!
python3 -m http.server 19093 --bind 127.0.0.1 -d "$tmpdir/b" > "$tmpdir/backend_b.log" 2>&1 & B=$!
./target/debug/arc-gateway --config "$tmpdir/arc_tls_sni.json" > "$tmpdir/gateway.log" 2>&1 & G=$!
sleep 2
R1=$(curl -sk --noproxy "*" --resolve web.local:18445:127.0.0.1 https://web.local:18445/)
R2=$(curl -sk --noproxy "*" --resolve api.local:18445:127.0.0.1 https://api.local:18445/)
kill $G $A $B >/dev/null 2>&1 || true
wait $G >/dev/null 2>&1 || true
wait $A >/dev/null 2>&1 || true
wait $B >/dev/null 2>&1 || true
echo "web.local=$R1"
echo "api.local=$R2"
