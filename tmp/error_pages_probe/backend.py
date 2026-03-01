from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path.endswith("/up502"):
            code, body = 502, b"upstream 502"
        elif path.endswith("/up504"):
            code, body = 504, b"upstream 504"
        elif path.endswith("/up404"):
            code, body = 404, b"upstream 404"
        elif path.endswith("/ok"):
            code, body = 200, b"ok"
        else:
            code, body = 200, b"default"

        self.send_response(code)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        return


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 19080), Handler).serve_forever()
