#!/usr/bin/env bash
set -euo pipefail
python3 - <<'PY'
import ssl
from http.server import BaseHTTPRequestHandler, HTTPServer
import threading
import subprocess

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        cert=self.connection.getpeercert()
        self.send_response(200 if cert else 401)
        self.end_headers()
        self.wfile.write(b'ok' if cert else b'no')
    def log_message(self,*a):
        pass

ctx=ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
ctx.verify_mode=ssl.CERT_REQUIRED
ctx.load_cert_chain('/tmp/arc_up_mtls/server.crt','/tmp/arc_up_mtls/server.key')
ctx.load_verify_locations(cafile='/tmp/arc_up_mtls/ca.crt')
srv=HTTPServer(('127.0.0.1',19107),H)
srv.socket=ctx.wrap_socket(srv.socket,server_side=True)
threading.Thread(target=srv.serve_forever,daemon=True).start()

out1=subprocess.check_output("curl -sk --cert /tmp/arc_up_mtls/client.crt --key /tmp/arc_up_mtls/client.key https://127.0.0.1:19107/ -w ' code=%{http_code}\\n'",shell=True,text=True)
out2=subprocess.check_output("curl -sk https://127.0.0.1:19107/ -w ' code=%{http_code}\\n' || true",shell=True,text=True)
print(out1)
print(out2)
srv.shutdown()
PY
