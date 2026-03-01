from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/x":
            body = b"REAL_BACKEND_X"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            body = ("REAL_BACKEND_" + self.path).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
    def log_message(self, fmt, *args):
        pass
HTTPServer(("127.0.0.1", 19096), H).serve_forever()
