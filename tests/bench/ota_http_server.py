#!/usr/bin/env python3
"""Controlled OTA artifact server for skylights bench testing.

Serves a directory containing a bare-integer ``version`` file, ``<v>.bin`` and
``<v>.bin.sha256`` (exactly the firmware's OTA contract), over HTTP or HTTPS,
optionally requiring HTTP Basic auth or serving an oversized ``.bin``.

Examples:
    python3 ota_http_server.py --dir root --port 8000
    python3 ota_http_server.py --dir root --port 8000 --oversize 2000000
    python3 ota_http_server.py --dir root --port 8443 --auth user:pass \\
        --tls-cert cert.pem --tls-key key.pem

The firmware uses no-verify TLS 1.3, so a self-signed certificate is fine.
"""
from __future__ import annotations

import argparse
import base64
import http.server
import ssl
import sys


def make_handler(directory: str, auth: str | None, oversize: int) -> type:
    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=directory, **kwargs)

        def _authorized(self) -> bool:
            if not auth:
                return True
            expected = "Basic " + base64.b64encode(auth.encode()).decode()
            return self.headers.get("Authorization", "") == expected

        def do_GET(self):  # noqa: N802 (stdlib naming)
            if not self._authorized():
                self.send_response(401)
                self.send_header("WWW-Authenticate", 'Basic realm="ota"')
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            if oversize > 0 and self.path.endswith(".bin"):
                body = b"\x00" * oversize
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            super().do_GET()

        def log_message(self, fmt, *args):
            sys.stderr.write("%s %s\n" % (self.address_string(), fmt % args))

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", required=True, help="artifact root directory")
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--bind", default="0.0.0.0")
    parser.add_argument("--auth", help="require this user:pass as Basic auth")
    parser.add_argument(
        "--oversize",
        type=int,
        default=0,
        help="serve every .bin as this many zero bytes (0 disables)",
    )
    parser.add_argument("--tls-cert")
    parser.add_argument("--tls-key")
    args = parser.parse_args()

    handler = make_handler(args.dir, args.auth, args.oversize)
    httpd = http.server.ThreadingHTTPServer((args.bind, args.port), handler)

    if args.tls_cert and args.tls_key:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.minimum_version = ssl.TLSVersion.TLSv1_3
        ctx.load_cert_chain(args.tls_cert, args.tls_key)
        httpd.socket = ctx.wrap_socket(httpd.socket, server_side=True)
        scheme = "https"
    elif args.tls_cert or args.tls_key:
        parser.error("--tls-cert and --tls-key must be given together")
    else:
        scheme = "http"

    print(
        f"serving {args.dir} on {scheme}://{args.bind}:{args.port}",
        file=sys.stderr,
    )
    httpd.serve_forever()


if __name__ == "__main__":
    main()
