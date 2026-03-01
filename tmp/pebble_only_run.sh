#!/usr/bin/env bash
set -u
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc || exit 1
pkill -x pebble >/dev/null 2>&1 || true
sleep 1
(cd tmp/pebble && PEBBLE_VA_NOSLEEP=1 PEBBLE_VA_ALWAYS_VALID=1 nohup ./pebble -config ./pebble-config.json > ../acme_smoke_pebble/pebble.log 2>&1 &)
sleep 2
echo '=== PEBBLE LISTEN ==='
ss -lntp | grep -E ':(14000|15000)\b' || true
echo '=== PEBBLE DIR ==='
curl -k -sS -m 5 https://localhost:14000/dir | head -c 260 || true; echo
echo '=== PEBBLE LOG ==='
tail -n 80 tmp/acme_smoke_pebble/pebble.log || true
