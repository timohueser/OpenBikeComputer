"""One local HTTP server per test, stopped when the test ends."""

from http.server import ThreadingHTTPServer
from threading import Thread


def serve(test, handler):
    """Serve `handler` on a free port until `test` is cleaned up; returns the server.

    A short poll interval keeps `shutdown` from waiting out the default half second.
    """

    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True).start()
    test.addCleanup(server.server_close)
    test.addCleanup(server.shutdown)
    return server
