#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc

WORK_DIR=/tmp/arc_up_mtls
PORT=19097

if [[ ! -f "${WORK_DIR}/backend.py" ]]; then
  echo "missing ${WORK_DIR}/backend.py"
  exit 1
fi

kill_port() {
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

kill_port "${PORT}"

python3 "${WORK_DIR}/backend.py" >"${WORK_DIR}/backend_test.log" 2>&1 &
BPID="$!"
trap 'kill "${BPID}" >/dev/null 2>&1 || true; wait "${BPID}" 2>/dev/null || true' EXIT

sleep 0.3

echo "[openssl with client cert]"
openssl s_client \
  -connect 127.0.0.1:${PORT} \
  -servername mtls.local \
  -cert "${WORK_DIR}/client.crt" \
  -key "${WORK_DIR}/client.key" \
  -CAfile "${WORK_DIR}/ca.crt" \
  -verify_return_error \
  < /dev/null 2>&1 | sed -n '1,80p'

echo "[curl with client cert]"
curl -svk --noproxy '*' \
  --connect-timeout 2 --max-time 5 \
  --cert "${WORK_DIR}/client.crt" \
  --key "${WORK_DIR}/client.key" \
  --cacert "${WORK_DIR}/ca.crt" \
  --resolve mtls.local:${PORT}:127.0.0.1 \
  "https://mtls.local:${PORT}/" \
  -o "${WORK_DIR}/backend_good.body" \
  -w " code=%{http_code}\n" 2>&1 | sed -n '1,120p' || true
echo "body=$(cat "${WORK_DIR}/backend_good.body" 2>/dev/null || true)"

echo "[python ssl client]"
python3 - <<'PY'
import ssl
import socket

ctx = ssl.create_default_context(ssl.Purpose.SERVER_AUTH, cafile="/tmp/arc_up_mtls/ca.crt")
ctx.check_hostname = False
ctx.load_cert_chain("/tmp/arc_up_mtls/client.crt", "/tmp/arc_up_mtls/client.key")

try:
    with socket.create_connection(("127.0.0.1", 19097), timeout=2) as s:
        with ctx.wrap_socket(s, server_hostname="mtls.local") as t:
            t.sendall(b"GET / HTTP/1.1\r\nHost: mtls.local\r\nConnection: close\r\n\r\n")
            data = b""
            while True:
                chunk = t.recv(4096)
                if not chunk:
                    break
                data += chunk
    print(data.decode("latin1", "ignore"))
except Exception as e:
    print(f"python client error: {e!r}")
PY

echo "[curl without client cert]"
curl -svk --noproxy '*' \
  --connect-timeout 2 --max-time 5 \
  --cacert "${WORK_DIR}/ca.crt" \
  --resolve mtls.local:${PORT}:127.0.0.1 \
  "https://mtls.local:${PORT}/" \
  -o "${WORK_DIR}/backend_bad.body" \
  -w " code=%{http_code}\n" 2>&1 | sed -n '1,120p' || true
echo "body=$(cat "${WORK_DIR}/backend_bad.body" 2>/dev/null || true)"
