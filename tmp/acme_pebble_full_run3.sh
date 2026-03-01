#!/usr/bin/env bash
set -u
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc || exit 1
mkdir -p tmp/acme_pebble_full
pkill -x arc-gateway >/dev/null 2>&1 || true
pkill -x pebble >/dev/null 2>&1 || true
sleep 1

# generate self-signed cert for Pebble HTTPS endpoint (localhost SAN)
openssl req -x509 -nodes -newkey rsa:2048 \
  -keyout tmp/acme_pebble_full/pebble_tls_key.pem \
  -out tmp/acme_pebble_full/pebble_tls_cert.pem \
  -days 2 -subj '/CN=localhost' \
  -addext 'subjectAltName=DNS:localhost' >/dev/null 2>&1

# create pebble config using custom TLS cert
cat > tmp/acme_pebble_full/pebble-config.local.json <<JSON
{
  "pebble": {
    "listenAddress": "0.0.0.0:14000",
    "managementListenAddress": "0.0.0.0:15000",
    "certificate": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/acme_pebble_full/pebble_tls_cert.pem",
    "privateKey": "/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/acme_pebble_full/pebble_tls_key.pem",
    "httpPort": 5002,
    "tlsPort": 5001,
    "ocspResponderURL": "",
    "externalAccountBindingRequired": false,
    "domainBlocklist": ["blocked-domain.example"],
    "retryAfter": {"authz": 3, "order": 5},
    "keyAlgorithm": "ecdsa",
    "profiles": {
      "default": {"description": "default", "validityPeriod": 7776000},
      "shortlived": {"description": "short", "validityPeriod": 518400}
    }
  }
}
JSON

# start pebble with real validation
PEBBLE_VA_NOSLEEP=1 nohup ./tmp/pebble/pebble -config ./tmp/acme_pebble_full/pebble-config.local.json > tmp/acme_pebble_full/pebble.log 2>&1 &
sleep 2

# check directory reachable with trusted cert file
curl --cacert tmp/acme_pebble_full/pebble_tls_cert.pem -sS -m 5 https://localhost:14000/dir >/dev/null || {
  echo 'PEBBLE_DIR_TLS_NOT_TRUSTED'
  tail -n 120 tmp/acme_pebble_full/pebble.log || true
  exit 1
}

# Arc config
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
      "directory_ca_pem": "tmp/acme_pebble_full/pebble_tls_cert.pem",
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

echo '=== PEBBLE ACME FLOW SIGNAL ==='
grep -Ei 'new-order|authz|challenge|finalize|certificate|valid|invalid' tmp/acme_pebble_full/pebble.log | tail -n 120 || true

echo '=== CACHE FILES ==='
find tmp/acme_pebble_full/cache -maxdepth 4 -type f -printf '%p\n' 2>/dev/null || true

echo '=== GATEWAY LOG ==='
tail -n 120 tmp/acme_pebble_full/gateway.log || true

echo '=== PEBBLE LOG ==='
tail -n 200 tmp/acme_pebble_full/pebble.log || true
