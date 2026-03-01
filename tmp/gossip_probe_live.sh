#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
TMP=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/gossip_two_node_probe
mkdir -p "$TMP"
A_CFG=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/gossip_two_node/node_a.json
A_CFG_V2=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/gossip_two_node/node_a_v2.json
B_CFG=/mnt/c/Users/Shuakami_Projects/CommunityProject/Arc/tmp/gossip_two_node/node_b.json
BACK_PY=$TMP/backend.py
cat > "$BACK_PY" <<'"'"'PY'"'"'
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body=b"ok\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, fmt, *args):
        return
HTTPServer(("127.0.0.1",19110), H).serve_forever()
PY
for p in 18601 18602 19601 19602 20101 20102 22101 22102 19110; do
  pids=$(ss -ltnp 2>/dev/null | grep -E "[\\.:]${p}[[:space:]]" | sed -n "s/.*pid=\([0-9]\+\).*/\1/p" | sort -u || true)
  if [ -n "$pids" ]; then kill -9 $pids || true; fi
done
python3 "$BACK_PY" > "$TMP/backend.log" 2>&1 & SPID=$!
./target/debug/arc-gateway --config "$A_CFG" > "$TMP/a.log" 2>&1 & APID=$!
./target/debug/arc-gateway --config "$B_CFG" > "$TMP/b.log" 2>&1 & BPID=$!
cleanup(){ set +e; kill $APID $BPID $SPID >/dev/null 2>&1 || true; wait $APID $BPID $SPID 2>/dev/null || true; }
trap cleanup EXIT
for i in $(seq 1 80); do
  curl -sS --noproxy "*" http://127.0.0.1:20101/v1/status >/dev/null 2>&1 && break
  sleep 0.1
done
for i in $(seq 1 80); do
  curl -sS --noproxy "*" http://127.0.0.1:20102/v1/status >/dev/null 2>&1 && break
  sleep 0.1
done
A0=$(curl -sS --noproxy "*" http://127.0.0.1:20101/v1/status | python3 -c '"'"'import json,sys;print(json.load(sys.stdin)["generation"])'"'"')
B0=$(curl -sS --noproxy "*" http://127.0.0.1:20102/v1/status | python3 -c '"'"'import json,sys;print(json.load(sys.stdin)["generation"])'"'"')
echo "before A=$A0 B=$B0"
RESP=$(curl -sS --noproxy "*" -H "Authorization: Bearer tkn" -H "Content-Type: application/json" --data-binary @"$A_CFG_V2" http://127.0.0.1:20101/v1/cluster/config)
NG=$(python3 -c '"'"'import json,sys;print(json.loads(sys.argv[1])["generation"])'"'"' "$RESP")
echo "resp=$RESP"
for i in $(seq 1 80); do
  A=$(curl -sS --noproxy "*" http://127.0.0.1:20101/v1/status | python3 -c '"'"'import json,sys;print(json.load(sys.stdin)["generation"])'"'"')
  B=$(curl -sS --noproxy "*" http://127.0.0.1:20102/v1/status | python3 -c '"'"'import json,sys;print(json.load(sys.stdin)["generation"])'"'"')
  echo "t=$i A=$A B=$B"
  sleep 0.2
done
