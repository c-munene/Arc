from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        host = self.headers.get("Host", "")
        body = f"backend-ok method=GET host={host} path={self.path}\n".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        host = self.headers.get("Host", "")
        length = int(self.headers.get("Content-Length", "0"))
        if length:
            self.rfile.read(length)
        body = f"backend-ok method=POST host={host} path={self.path}\n".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

HTTPServer(("127.0.0.1", 19080), H).serve_forever()
