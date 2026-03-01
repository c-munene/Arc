#!/usr/bin/env bash
set -u
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc || exit 1
mkdir -p tmp/acme_smoke_pebble tmp/pebble
pkill -x arc-gateway >/dev/null 2>&1 || true
pkill -x pebble >/dev/null 2>&1 || true
pkill -f 'python3 -m http.server 19000' >/dev/null 2>&1 || true

# start backend
nohup python3 -m http.server 19000 --directory tmp/acme_smoke > tmp/acme_smoke_pebble/backend.log 2>&1 &

# start pebble (no challenge sleep, always valid)
PEBBLE_VA_NOSLEEP=1 PEBBLE_VA_ALWAYS_VALID=1 nohup ./tmp/pebble/pebble -config ./tmp/pebble/pebble-config.json > tmp/acme_smoke_pebble/pebble.log 2>&1 &
sleep 2

# write pebble config variant
cat > tmp/acme_smoke_pebble/arc_acme_pebble.json <<'JSON'
{
  "listen": "127.0.0.1:18443",
  "admin_listen": "127.0.0.1:19900",
  "control_plane": {
    "enabled": false,
    "bind": "127.0.0.1:19998",
    "role": "standalone",
    "node_id": "arc-node-1",
    "peers": [],
    "quorum": 0,
    "auth_token": null,
    "pull_from": null,
    "pull_interval_ms": 1000,
    "peer_timeout_ms": 1200
  },
  "global_rate_limit": {
    "backend": "in_memory",
    "redis": {
      "url": "redis://127.0.0.1:6379/0",
      "budget_ms": 2,
      "circuit_open_ms": 500,
      "prefetch": 128,
      "low_watermark": 16,
      "refill_backoff_ms": 1
    }
  },
  "cluster_circuit": {
    "failure_threshold": 8,
    "circuit_open_ms": 3000,
    "quorum": 1,
    "half_open_probe_interval_ms": 200
  },
  "workers": 1,
  "linger_ms": 300,
  "io_uring": {
    "entries": 1024,
    "accept_multishot": true,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "buffers": {
    "buf_size": 8192,
    "buf_count": 4096
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
  "upstreams": [
    {
      "name": "default",
      "addr": "127.0.0.1:19000",
      "keepalive": 128,
      "idle_ttl_ms": 30000
    }
  ],
  "plugins": [],
  "routes": [
    {
      "path": "/",
      "upstream": "default",
      "plugins": [],
      "rate_limit": { "rps": 100000, "burst": 200000 }
    }
  ],
  "downstream_tls": {
    "enable_h2": false,
    "certificates": [
      {
        "sni": "localhost",
        "cert_pem": "tmp/acme_smoke/static_cert.pem",
        "key_pem": "tmp/acme_smoke/static_key.pem"
      }
    ],
    "sni_routes": [],
    "acme": {
      "enabled": true,
      "directory_url": "https://localhost:14000/dir",
      "email": "ops@example.com",
      "account_key": {
        "algorithm": "ed25519",
        "encrypted_key_path": "tmp/acme_smoke_pebble/account.enc",
        "passphrase": { "type": "env", "name": "ARC_ACME_PASS" }
      },
      "challenge_priority": ["http01"],
      "http01": { "listen": "127.0.0.1:18080" },
      "poll_interval_secs": 5,
      "members": [],
      "certificates": [
        {
          "domain": "acme.local",
          "cert_pem": "tmp/acme_smoke_pebble/acme_cert.pem",
          "key_pem": "tmp/acme_smoke_pebble/acme_key.pem"
        }
      ],

      "domains": ["acme.local"],
      "staging": true,
      "cache_dir": "tmp/acme_smoke_pebble/cache",
      "master_key_env": "ARC_MASTER_KEY",
      "challenge": "http-01",
      "http01_listen": "127.0.0.1:18080",
      "startup_jitter_secs": 0,
      "runtime_threads": 1
    }
  }
}
JSON

# create placeholder managed cert files for config compile path
cp -f tmp/acme_smoke/acme_cert.pem tmp/acme_smoke_pebble/acme_cert.pem
cp -f tmp/acme_smoke/acme_key.pem tmp/acme_smoke_pebble/acme_key.pem

# start gateway
ARC_MASTER_KEY='base64:MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=' ARC_ACME_PASS='pass123' nohup ./target/debug/arc-gateway --config tmp/acme_smoke_pebble/arc_acme_pebble.json > tmp/acme_smoke_pebble/gateway.log 2>&1 &

sleep 8

echo '=== PEBBLE LISTEN ==='
ss -lntp | grep -E ':(14000|15000)\b' || true
echo '=== PEBBLE DIR (curl -k) ==='
curl -k -sS -m 3 https://localhost:14000/dir | head -c 260 || true; echo
echo '=== ACME CACHE FILES ==='
find tmp/acme_smoke_pebble/cache -maxdepth 4 -type f -printf '%p\n' 2>/dev/null || true
echo '=== GATEWAY LOG ==='
tail -n 120 tmp/acme_smoke_pebble/gateway.log || true
echo '=== PEBBLE LOG ==='
tail -n 120 tmp/acme_smoke_pebble/pebble.log || true
