"""Serves a built site on loopback for the browser suites, as `python3 -m http.server` does."""

import functools
import sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer


class Server(ThreadingHTTPServer):
    # The default listen queue is 5. On macOS, a page that loads many modules at once overflows it
    # and the browser gets ERR_CONNECTION_RESET.
    request_queue_size = 128


class Handler(SimpleHTTPRequestHandler):
    def log_message(self, format, *args):
        pass


port, directory = sys.argv[1:]
Server(("127.0.0.1", int(port)), functools.partial(Handler, directory=directory)).serve_forever()
