#!/usr/bin/env bash
set -u
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc || exit 1
pkill -x arc-gateway >/dev/null 2>&1 || true
sleep 1
ARC_MASTER_KEY='base64:MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=' ARC_ACME_PASS='pass123' nohup ./target/debug/arc-gateway --config tmp/acme_smoke_pebble/arc_acme_pebble.json > tmp/acme_smoke_pebble/gateway.log 2>&1 &
echo $! > tmp/acme_smoke_pebble/gateway.pid
sleep 10
echo '=== GATEWAY LISTEN ==='
ss -lntp | grep -E ':(18443|18080)\b' || true
echo '=== PEBBLE LOG (LAST 120) ==='
tail -n 120 tmp/acme_smoke_pebble/pebble.log || true
echo '=== GATEWAY LOG (LAST 120) ==='
tail -n 120 tmp/acme_smoke_pebble/gateway.log || true
