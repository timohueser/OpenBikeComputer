"""Serve the speed/ directory for the browser bench and collect the results it POSTs.

    python3 web/serve.py [--port 8765]
    open "http://localhost:8765/web/index.html"            # runs the whole plan, no clicks

Every response carries COOP/COEP, so the page is cross-origin isolated (WASM threads) on
localhost. Over the LAN (plain http, not a secure context) browsers do not isolate the page,
and ORT falls back to 1 thread; --no-isolation reproduces that on localhost.
Results go to work/web_results.jsonl.

iPhone on the same Wi-Fi (the model fp32 file is too large for a phone tab, so skip it):
    python3 web/serve.py            # prints the LAN URL
    http://<mac-lan-ip>:8765/web/index.html?plan=ort:1:cold,ort:1:cached,rust:1:model.int8.onnx
"""
import argparse
import errno
import json
import socket
import time
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SPEED = Path(__file__).resolve().parent.parent
RESULTS = SPEED / "work" / "web_results.jsonl"


class Handler(SimpleHTTPRequestHandler):
    extensions_map = {**SimpleHTTPRequestHandler.extensions_map, ".mjs": "text/javascript",
                      ".wasm": "application/wasm", ".onnx": "application/octet-stream"}

    isolate = True

    def end_headers(self):
        if self.isolate:
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
            self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        if self.path.split("?")[0].endswith((".html", "/")) or "bench" in self.path:
            self.send_header("Cache-Control", "no-store")
        else:
            self.send_header("Cache-Control", "public, max-age=86400")
        super().end_headers()

    def copyfile(self, source, outputfile):
        # macOS localhost sockets raise ENOBUFS on large bursts; retry the chunk instead of dropping the file.
        while chunk := source.read(1 << 20):
            view = memoryview(chunk)
            while view:
                try:
                    view = view[self.connection.send(view):]
                except OSError as e:
                    if e.errno != errno.ENOBUFS:
                        raise
                    time.sleep(0.01)

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        row = json.loads(body)
        row["received"] = time.strftime("%H:%M:%S")
        with RESULTS.open("a") as f:
            f.write(json.dumps(row) + "\n")
        print("RESULT", json.dumps(row), flush=True)
        self.send_response(204)
        self.end_headers()

    def log_message(self, *a):
        pass


def lan_ip():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("192.0.2.1", 9))
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--no-isolation", action="store_true", help="omit COOP/COEP, as a phone on the LAN sees it")
    args = ap.parse_args()
    Handler.isolate = not args.no_isolation
    RESULTS.parent.mkdir(exist_ok=True)
    print(f"http://localhost:{args.port}/web/index.html  (LAN: http://{lan_ip()}:{args.port}/web/index.html)", flush=True)
    ThreadingHTTPServer(("0.0.0.0", args.port), partial(Handler, directory=str(SPEED))).serve_forever()
