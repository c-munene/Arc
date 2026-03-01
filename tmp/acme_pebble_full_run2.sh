#!/usr/bin/env bash
set -u
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc || exit 1
mkdir -p tmp/acme_pebble_full
pkill -x arc-gateway >/dev/null 2>&1 || true
pkill -x pebble >/dev/null 2>&1 || true
sleep 1

# start pebble
(cd tmp/pebble && PEBBLE_VA_NOSLEEP=1 nohup ./pebble -config ./pebble-config.json > ../acme_pebble_full/pebble.log 2>&1 &)
sleep 2

# fetch live root cert exposed by pebble management API
curl -k -sS https://localhost:15000/roots/0 -o tmp/acme_pebble_full/pebble-root.pem
if [ ! -s tmp/acme_pebble_full/pebble-root.pem ]; then
  echo 'FAILED_FETCH_PEBBLE_ROOT'
  tail -n 120 tmp/acme_pebble_full/pebble.log || true
  exit 1
fi

# write config with dynamic root CA file
cat > tmp/acme_pebble_full/arc.json <<'JSON'
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
      "directory_ca_pem": "tmp/acme_pebble_full/pebble-root.pem",
      "email": "ops@example.com",
      "domains": ["localhost"],
      "staging": true,
      "cache_dir": "tmp/acme_pebble_full/cache",
      "master_key_env": "ARC_MASTER_KEY",
      "account_key": {
        "algorithm": "ed25519",
        "encrypted_key_path": "tmp/acme_pebble_full/account.enc",
        "passphrase": { "type": "env", "name": "ARC_ACME_PASS" }
      },
      "challenge": "http-01",
      "http01_listen": "127.0.0.1:5002",
      "challenge_priority": ["http01"],
      "http01": { "listen": "127.0.0.1:5002" },
      "poll_interval_secs": 3,
      "startup_jitter_secs": 0,
      "runtime_threads": 1,
      "members": [],
      "certificates": [
        {
          "domain": "localhost",
          "cert_pem": "tmp/acme_pebble_full/acme_cert.pem",
          "key_pem": "tmp/acme_pebble_full/acme_key.pem"
        }
      ]
    }
  }
}
JSON

cp -f tmp/acme_smoke/acme_cert.pem tmp/acme_pebble_full/acme_cert.pem
cp -f tmp/acme_smoke/acme_key.pem tmp/acme_pebble_full/acme_key.pem

ARC_MASTER_KEY='base64:MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=' ARC_ACME_PASS='pass123' nohup ./target/debug/arc-gateway --config tmp/acme_pebble_full/arc.json > tmp/acme_pebble_full/gateway.log 2>&1 &
sleep 3
before_issuer=$(echo | openssl s_client -connect 127.0.0.1:18443 -servername localhost 2>/dev/null | openssl x509 -noout -issuer | sed 's/^issuer=//')

issued=0
for i in $(seq 1 60); do
  sleep 2
  issuer=$(echo | openssl s_client -connect 127.0.0.1:18443 -servername localhost 2>/dev/null | openssl x509 -noout -issuer | sed 's/^issuer=//')
  if echo "$issuer" | grep -qi 'Pebble'; then
    issued=1
    break
  fi
done
after_issuer=$(echo | openssl s_client -connect 127.0.0.1:18443 -servername localhost 2>/dev/null | openssl x509 -noout -issuer | sed 's/^issuer=//')

echo '=== ACCEPTANCE ==='
echo "ISSUER_BEFORE=$before_issuer"
echo "ISSUER_AFTER=$after_issuer"
echo "ISSUED_BY_PEBBLE=$issued"
if grep -qi 'unknown certificate authority' tmp/acme_pebble_full/pebble.log; then
  echo 'UNKNOWN_CA=YES'
else
  echo 'UNKNOWN_CA=NO'
fi

echo '=== PEBBLE CHALLENGE/AUTHZ SIGNAL ==='
grep -Ei 'challenge|authz|order|finalize|valid|invalid' tmp/acme_pebble_full/pebble.log | tail -n 80 || true

echo '=== ACME CACHE FILES ==='
find tmp/acme_pebble_full/cache -maxdepth 4 -type f -printf '%p\n' 2>/dev/null || true

echo '=== GATEWAY LOG TAIL ==='
tail -n 120 tmp/acme_pebble_full/gateway.log || true

echo '=== PEBBLE LOG TAIL ==='
tail -n 180 tmp/acme_pebble_full/pebble.log || true
