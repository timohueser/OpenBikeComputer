"""Serve a map benchmark and capture exact overlay replies for offline replay."""
import argparse
import hashlib
import json
import mimetypes
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("assets", type=Path)
    parser.add_argument("maps", type=Path)
    parser.add_argument("cutout", type=Path)
    parser.add_argument("--port", type=int, default=8795)
    parser.add_argument("--capture", help="Overlay API origin, for example http://127.0.0.1:8787")
    args = parser.parse_args()
    assets = args.assets.resolve()
    cache = assets / "overlays"
    cache.mkdir(exist_ok=True)
    roots = {"/map-benchmark/": assets, "/maps/": args.maps.resolve(), "/map-cutout/": args.cutout.resolve()}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            if self.path != "/report":
                self.send_error(404)
                return
            (assets / "result.json").write_bytes(self.rfile.read(int(self.headers["Content-Length"])))
            self.send_response(204)
            self.end_headers()

        def do_GET(self):
            try:
                if self.path.startswith("/routing/v1/overlays?"):
                    path = cache / (hashlib.sha256(self.path.encode()).hexdigest() + ".json")
                    if not path.exists():
                        if not args.capture:
                            raise FileNotFoundError("No captured reply for this exact query")
                        url = args.capture.rstrip("/") + self.path.removeprefix("/routing")
                        with urllib.request.urlopen(url, timeout=60) as response:
                            path.write_bytes(response.read())
                        with (cache / "requests.jsonl").open("a") as stream:
                            stream.write(json.dumps({"url": self.path, "file": path.name}) + "\n")
                else:
                    prefix = next(value for value in roots if self.path.startswith(value))
                    root = roots[prefix]
                    path = (root / urllib.parse.unquote(self.path[len(prefix):].split("?")[0])).resolve()
                    if not path.is_relative_to(root):
                        raise ValueError("Path escapes the asset directory")
                size = path.stat().st_size
                start, end, status = 0, size - 1, 200
                if self.headers.get("Range"):
                    begin, finish = self.headers["Range"].removeprefix("bytes=").split("-")
                    start, end, status = int(begin), min(int(finish) if finish else end, end), 206
                if not 0 <= start <= end < size:
                    raise ValueError("Invalid range")
                self.send_response(status)
                self.send_header("Content-Type", mimetypes.guess_type(path)[0] or "application/octet-stream")
                self.send_header("Content-Length", str(end - start + 1))
                self.send_header("Cache-Control", "no-store")
                self.send_header("Accept-Ranges", "bytes")
                if status == 206:
                    self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
                self.end_headers()
                with path.open("rb") as stream:
                    stream.seek(start)
                    self.wfile.write(stream.read(end - start + 1))
            except (BrokenPipeError, ConnectionResetError):
                pass
            except Exception as error:
                self.send_error(404, str(error))
                print(f"{self.path}: {error}", flush=True)

    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
