#!/usr/bin/env python3
import http.server
import socketserver
import sys

port = int(sys.argv[1])
body = sys.argv[2].encode('utf-8')

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/plain')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, fmt, *args):
        return

with socketserver.TCPServer(('127.0.0.1', port), H) as s:
    s.serve_forever()
