from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path.startswith("/__arc/error/"):
            status = self.headers.get("X-Arc-Error-Status", "")
            source = self.headers.get("X-Arc-Error-Source", "")
            rid = self.headers.get("X-Arc-Request-Id", "")
            upstream = self.headers.get("X-Arc-Upstream", "")
            body = (
                f"error-service status={status} source={source} "
                f"request_id={rid} upstream={upstream}"
            ).encode("utf-8")
            code = 200
        else:
            body = b"error-service default"
            code = 200

        self.send_response(code)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        return


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 19081), Handler).serve_forever()
