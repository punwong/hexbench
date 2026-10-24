"""Tiny WebDAV stand-in for HexBench sync testing.

Supports PROPFIND (207), GET, PUT and DELETE over HTTP Basic auth on a flat
directory. Not a real WebDAV server: enough to validate the client round trip.

    python dav-server.py [port]
"""

import base64
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "dav-root")
USER = os.environ.get("DAV_USER", "test")
PASSWORD = os.environ.get("DAV_PASS", "123456")

os.makedirs(ROOT, exist_ok=True)


def local_path(path: str) -> str:
    return os.path.join(ROOT, os.path.basename(path))


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def authorized(self) -> bool:
        header = self.headers.get("Authorization", "")
        expected = "Basic " + base64.b64encode(f"{USER}:{PASSWORD}".encode()).decode()
        if header == expected:
            return True
        self.send_response(401)
        self.send_header("WWW-Authenticate", 'Basic realm="hexbench"')
        self.send_header("Content-Length", "0")
        self.end_headers()
        return False

    def do_PROPFIND(self):
        if not self.authorized():
            return
        body = (
            '<?xml version="1.0" encoding="utf-8"?>'
            '<d:multistatus xmlns:d="DAV:"><d:response><d:href>/</d:href>'
            "<d:propstat><d:prop><d:resourcetype><d:collection/>"
            "</d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status>"
            "</d:propstat></d:response></d:multistatus>"
        ).encode()
        self.send_response(207)
        self.send_header("Content-Type", "application/xml; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_PUT(self):
        if not self.authorized():
            return
        length = int(self.headers.get("Content-Length", "0"))
        data = self.rfile.read(length)
        with open(local_path(self.path), "wb") as handle:
            handle.write(data)
        self.send_response(201)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):
        if not self.authorized():
            return
        path = local_path(self.path)
        if not os.path.isfile(path):
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        with open(path, "rb") as handle:
            data = handle.read()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_DELETE(self):
        if not self.authorized():
            return
        path = local_path(self.path)
        if os.path.isfile(path):
            os.remove(path)
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, fmt, *args):
        sys.stderr.write("dav: " + fmt % args + "\n")


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    print(f"serving {ROOT} on http://127.0.0.1:{port}/ (user {USER})", flush=True)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
