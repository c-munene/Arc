#!/usr/bin/env python3
import argparse
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Simple benchmark backend")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--name", default="backend")
    parser.add_argument("--status", type=int, default=200)
    parser.add_argument("--payload-bytes", type=int, default=2)
    parser.add_argument("--delay-ms", type=int, default=0)
    return parser.parse_args()


def build_payload(size: int) -> bytes:
    if size <= 0:
        return b""
    return b"x" * size


def main() -> None:
    args = parse_args()
    payload = build_payload(args.payload_bytes)

    class Handler(BaseHTTPRequestHandler):
        server_version = "bench-backend/1.0"
        sys_version = ""

        def _handle(self) -> None:
            if args.delay_ms > 0:
                time.sleep(args.delay_ms / 1000.0)
            self.send_response(args.status)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("X-Benchmark-Backend", args.name)
            self.end_headers()
            if payload:
                self.wfile.write(payload)

        def do_GET(self) -> None:
            self._handle()

        def do_POST(self) -> None:
            self._handle()

        def do_PUT(self) -> None:
            self._handle()

        def do_DELETE(self) -> None:
            self._handle()

        def do_HEAD(self) -> None:
            if args.delay_ms > 0:
                time.sleep(args.delay_ms / 1000.0)
            self.send_response(args.status)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("X-Benchmark-Backend", args.name)
            self.end_headers()

        def log_message(self, format: str, *args_) -> None:
            return

    server = ThreadingHTTPServer((args.host, args.port), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()

