#!/usr/bin/env bash
set -u
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc || exit 1
pkill -x arc-gateway >/dev/null 2>&1 || true
pkill -f 'python3 -m http.server 19000' >/dev/null 2>&1 || true
nohup python3 -m http.server 19000 --directory tmp/acme_smoke > tmp/acme_smoke/backend.log 2>&1 &
ARC_MASTER_KEY='base64:MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=' ARC_ACME_PASS='pass123' nohup ./target/debug/arc-gateway --config tmp/acme_smoke/arc_acme_smoke_route.json > tmp/acme_smoke/gateway_route.log 2>&1 &
sleep 5
echo '=== HTTPS challenge via gateway ==='
curl -k -sS -m 5 -i https://127.0.0.1:18443/.well-known/acme-challenge/test | sed -n '1,20p' || true
echo '=== HTTP01 direct ==='
curl -sS -m 3 -i http://127.0.0.1:18080/.well-known/acme-challenge/test | sed -n '1,20p' || true
echo '=== gateway log ==='
tail -n 60 tmp/acme_smoke/gateway_route.log || true
