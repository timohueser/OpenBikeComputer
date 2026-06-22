#!/usr/bin/env python3
"""Tiny static file server for previewing the rendered docs locally.

`python3 docs/serve.py <abs-dir> <port>` serves <abs-dir> on 127.0.0.1:<port>.
Deliberately avoids os.getcwd()/Path.resolve() (some sandboxes block getcwd), so
it takes an absolute directory as an argument instead of inferring one.
"""
import functools
import http.server
import socketserver
import sys

directory = sys.argv[1]
port = int(sys.argv[2]) if len(sys.argv) > 2 else 8090

Handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=directory)
socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("127.0.0.1", port), Handler) as httpd:
    print("serving %s on http://127.0.0.1:%d" % (directory, port))
    httpd.serve_forever()
