from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        xr = self.headers.get('x-route', '')
        body = f"u1 path={self.path} x-route={xr}\\n".encode()
        self.send_response(200)
        self.send_header('Content-Type','text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        return
HTTPServer(('127.0.0.1', 19080), H).serve_forever()
