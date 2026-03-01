#!/usr/bin/env bash
set -u
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc || exit 1
mkdir -p tmp/acme_smoke
pkill -x arc-gateway >/dev/null 2>&1 || true
pkill -f 'python3 -m http.server 19000' >/dev/null 2>&1 || true
nohup python3 -m http.server 19000 --directory tmp/acme_smoke > tmp/acme_smoke/backend.log 2>&1 &
echo $! > tmp/acme_smoke/backend.pid
ARC_MASTER_KEY='base64:MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=' ARC_ACME_PASS='pass123' nohup ./target/debug/arc-gateway --config tmp/acme_smoke/arc_acme_smoke.json > tmp/acme_smoke/gateway.log 2>&1 &
echo $! > tmp/acme_smoke/gateway.pid
sleep 5
echo '=== PIDS ==='
cat tmp/acme_smoke/gateway.pid tmp/acme_smoke/backend.pid
echo '=== LISTEN ==='
ss -lntp | grep -E ':(18443|18080|19000)\b' || true
echo '=== HTTP01 CHALLENGE ==='
curl -sS -m 3 -i http://127.0.0.1:18080/.well-known/acme-challenge/test | sed -n '1,16p' || true
echo '=== HTTP01 NON-CHALLENGE ==='
curl -sS -m 3 -i http://127.0.0.1:18080/ | sed -n '1,16p' || true
echo '=== GATEWAY HTTPS CHALLENGE PATH ==='
curl -k -sS -m 5 -i https://127.0.0.1:18443/.well-known/acme-challenge/test | sed -n '1,16p' || true
echo '=== ACME FILES ==='
ls -l tmp/acme_smoke/account.enc 2>/dev/null || true
find tmp/acme_smoke/cache -maxdepth 3 -type f -printf '%p\n' 2>/dev/null || true
echo '=== GATEWAY LOG TAIL ==='
tail -n 120 tmp/acme_smoke/gateway.log || true
