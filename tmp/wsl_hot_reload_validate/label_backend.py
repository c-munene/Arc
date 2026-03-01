from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import sys

label = sys.argv[1].encode("utf-8")
port = int(sys.argv[2])

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = label + b"\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
