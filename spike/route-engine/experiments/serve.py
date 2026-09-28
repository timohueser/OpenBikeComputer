"""Serve local experiment files with an optional fixed delay before each GET."""

import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import time


class Handler(SimpleHTTPRequestHandler):
    delay_seconds = 0.0

    def do_GET(self):
        time.sleep(self.delay_seconds)
        super().do_GET()

    def log_message(self, format, *args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path.cwd())
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--delay-ms", type=float, default=0)
    args = parser.parse_args()
    if not args.directory.is_dir():
        parser.error("--directory must name an existing directory")
    if not 0 <= args.delay_ms < float("inf"):
        parser.error("--delay-ms must be finite and nonnegative")
    if not 0 <= args.port <= 65535:
        parser.error("--port must be between 0 and 65535")
    Handler.delay_seconds = args.delay_ms / 1000
    handler = partial(Handler, directory=str(args.directory.resolve()))
    with ThreadingHTTPServer(("127.0.0.1", args.port), handler) as server:
        print(f"Serving {args.directory.resolve()} at http://127.0.0.1:{server.server_port}", flush=True)
        print(f"Controlled HTTP/1 server delay: {args.delay_ms:g} ms per GET. "
              "This does not simulate WAN RTT or bandwidth. No route computation or uploads.", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
