#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

ARC_PORT=18081
UP_PORT=19097
ADMIN_PORT=19999
WORK_DIR=/tmp/arc_up_mtls
APID=""
BPID=""

kill_port_pids() {
  local port="$1"
  local pids
  pids="$(
    ss -ltnp 2>/dev/null | awk -v p=":${port}" '
      $4 ~ p {
        if (match($0, /pid=[0-9]+/)) {
          print substr($0, RSTART + 4, RLENGTH - 4)
        }
      }' | sort -u
  )"
  if [[ -n "${pids}" ]]; then
    # shellcheck disable=SC2086
    kill -9 ${pids} >/dev/null 2>&1 || true
  fi
}

cleanup_ports() {
  kill_port_pids "$ARC_PORT"
  kill_port_pids "$UP_PORT"
  kill_port_pids "$ADMIN_PORT"
}

wait_http_ready() {
  local url="$1"
  local max_tries="${2:-40}"
  local i
  for ((i = 0; i < max_tries; i++)); do
    if curl -sS --connect-timeout 1 --max-time 1 "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

wait_port_listen() {
  local port="$1"
  local max_tries="${2:-40}"
  local i
  for ((i = 0; i < max_tries; i++)); do
    if ss -ltn 2>/dev/null | awk '{print $4}' | grep -q ":${port}$"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

cleanup() {
  if [[ -n "${APID}" ]]; then
    kill "${APID}" >/dev/null 2>&1 || true
    wait "${APID}" 2>/dev/null || true
  fi
  if [[ -n "${BPID}" ]]; then
    kill "${BPID}" >/dev/null 2>&1 || true
    wait "${BPID}" 2>/dev/null || true
  fi
  cleanup_ports
}
trap cleanup EXIT

cleanup_ports
rm -rf "${WORK_DIR}"
mkdir -p "${WORK_DIR}"

openssl genrsa -out "${WORK_DIR}/ca.key" 2048 >/dev/null 2>&1
openssl req -new -key "${WORK_DIR}/ca.key" -subj "/CN=ArcTestCA" \
  -out "${WORK_DIR}/ca.csr" >/dev/null 2>&1
cat > "${WORK_DIR}/ca.ext" <<'EOF'
basicConstraints=critical,CA:TRUE,pathlen:1
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid:always,issuer
EOF
openssl x509 -req -in "${WORK_DIR}/ca.csr" -signkey "${WORK_DIR}/ca.key" \
  -out "${WORK_DIR}/ca.crt" -days 1 -sha256 -extfile "${WORK_DIR}/ca.ext" >/dev/null 2>&1

openssl genrsa -out "${WORK_DIR}/server.key" 2048 >/dev/null 2>&1
openssl req -new -key "${WORK_DIR}/server.key" -subj "/CN=mtls.local" \
  -out "${WORK_DIR}/server.csr" >/dev/null 2>&1
cat > "${WORK_DIR}/server.ext" <<'EOF'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
subjectAltName=DNS:mtls.local,IP:127.0.0.1
extendedKeyUsage=serverAuth
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid,issuer
EOF
openssl x509 -req -in "${WORK_DIR}/server.csr" \
  -CA "${WORK_DIR}/ca.crt" -CAkey "${WORK_DIR}/ca.key" -CAcreateserial \
  -out "${WORK_DIR}/server.crt" -days 1 -sha256 -extfile "${WORK_DIR}/server.ext" >/dev/null 2>&1

openssl genrsa -out "${WORK_DIR}/client.key" 2048 >/dev/null 2>&1
openssl req -new -key "${WORK_DIR}/client.key" -subj "/CN=arc-client" \
  -out "${WORK_DIR}/client.csr" >/dev/null 2>&1
cat > "${WORK_DIR}/client.ext" <<'EOF'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=clientAuth
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid,issuer
EOF
openssl x509 -req -in "${WORK_DIR}/client.csr" \
  -CA "${WORK_DIR}/ca.crt" -CAkey "${WORK_DIR}/ca.key" -CAcreateserial \
  -out "${WORK_DIR}/client.crt" -days 1 -sha256 -extfile "${WORK_DIR}/client.ext" >/dev/null 2>&1

cat > "${WORK_DIR}/backend.py" <<'PY'
import ssl
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        cert = self.connection.getpeercert()
        if not cert:
            self.send_response(401)
            self.end_headers()
            self.wfile.write(b"no-client-cert")
            return
        self.send_response(200)
        self.send_header("content-length", "16")
        self.end_headers()
        self.wfile.write(b"mtls-upstream-ok")
    def log_message(self, fmt, *args):
        pass

ctx = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
ctx.verify_mode = ssl.CERT_REQUIRED
ctx.load_cert_chain("/tmp/arc_up_mtls/server.crt", "/tmp/arc_up_mtls/server.key")
ctx.load_verify_locations(cafile="/tmp/arc_up_mtls/ca.crt")
srv = HTTPServer(("127.0.0.1", 19097), H)
srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
srv.serve_forever()
PY

cat > "${WORK_DIR}/arc.json" <<'JSON'
{
  "listen": "127.0.0.1:18081",
  "admin_listen": "127.0.0.1:19999",
  "workers": 1,
  "linger_ms": 100,
  "io_uring": {
    "entries": 256,
    "accept_multishot": false,
    "accept_prepost": 16,
    "tick_ms": 10,
    "sqpoll": false,
    "sqpoll_idle_ms": 0,
    "iopoll": false
  },
  "listen_backlog": 1024,
  "buffers": {"buf_size": 8192, "buf_count": 256},
  "timeouts_ms": {
    "cli_handshake": 1000,
    "cli_read": 10000,
    "up_conn": 1000,
    "up_handshake": 2000,
    "up_write": 10000,
    "up_read": 10000,
    "cli_write": 10000
  },
  "require_upstream_mtls": true,
  "upstreams": [{
    "name": "u",
    "addr": "127.0.0.1:19097",
    "keepalive": 16,
    "idle_ttl_ms": 5000,
    "tls": {
      "server_name": "mtls.local",
      "ca_pem": "/tmp/arc_up_mtls/ca.crt",
      "client_cert_pem": "/tmp/arc_up_mtls/client.crt",
      "client_key_pem": "/tmp/arc_up_mtls/client.key",
      "insecure_skip_verify": false,
      "enable_resumption": true
    }
  }],
  "plugins": [],
  "routes": [{"path": "/", "upstream": "u", "plugins": [], "rate_limit": null}]
}
JSON

python3 "${WORK_DIR}/backend.py" >"${WORK_DIR}/backend.log" 2>&1 &
BPID="$!"

if ! wait_port_listen "${UP_PORT}" 80; then
  echo "backend did not start in time"
  if [[ -n "${BPID}" ]] && ! kill -0 "${BPID}" 2>/dev/null; then
    wait "${BPID}" 2>/dev/null || true
  fi
  tail -n 120 "${WORK_DIR}/backend.log" || true
  exit 1
fi

./target/release/arc-gateway --config "${WORK_DIR}/arc.json" >"${WORK_DIR}/arc.log" 2>&1 &
APID="$!"

if ! wait_http_ready "http://127.0.0.1:${ARC_PORT}/"; then
  echo "arc did not start in time"
  tail -n 80 "${WORK_DIR}/arc.log" || true
  exit 1
fi

GOOD_CODE="$(curl -sS --connect-timeout 2 --max-time 5 -o "${WORK_DIR}/good.body" -w "%{http_code}" "http://127.0.0.1:${ARC_PORT}/")"
GOOD_BODY="$(cat "${WORK_DIR}/good.body" || true)"

echo "GOOD_CODE=${GOOD_CODE}"
echo "GOOD_BODY=${GOOD_BODY}"

if [[ "${GOOD_CODE}" != "200" || "${GOOD_BODY}" != "mtls-upstream-ok" ]]; then
  echo "mtls positive check failed"
  echo "--- arc.log ---"
  tail -n 120 "${WORK_DIR}/arc.log" || true
  echo "--- backend.log ---"
  tail -n 120 "${WORK_DIR}/backend.log" || true
  exit 1
fi

# Negative check: wrong upstream CA should fail verification and return non-200.
cp "${WORK_DIR}/server.crt" "${WORK_DIR}/bad_ca.crt"
sed 's#"/tmp/arc_up_mtls/ca.crt"#"/tmp/arc_up_mtls/bad_ca.crt"#g' "${WORK_DIR}/arc.json" > "${WORK_DIR}/arc_bad.json"

kill "${APID}" >/dev/null 2>&1 || true
wait "${APID}" 2>/dev/null || true
APID=""

./target/release/arc-gateway --config "${WORK_DIR}/arc_bad.json" >"${WORK_DIR}/arc_bad.log" 2>&1 &
APID="$!"

if ! wait_http_ready "http://127.0.0.1:${ARC_PORT}/"; then
  echo "arc(bad-ca) did not start in time"
  tail -n 80 "${WORK_DIR}/arc_bad.log" || true
  exit 1
fi

BAD_CODE="$(curl -sS --connect-timeout 2 --max-time 5 -o "${WORK_DIR}/bad.body" -w "%{http_code}" "http://127.0.0.1:${ARC_PORT}/" || true)"
BAD_BODY="$(cat "${WORK_DIR}/bad.body" || true)"

echo "BAD_CODE=${BAD_CODE}"
echo "BAD_BODY=${BAD_BODY}"

if [[ "${BAD_CODE}" == "200" ]]; then
  echo "mtls negative check failed"
  echo "--- arc_bad.log ---"
  tail -n 120 "${WORK_DIR}/arc_bad.log" || true
  exit 1
fi

echo "UPSTREAM_MTLS_SMOKE=PASS"
