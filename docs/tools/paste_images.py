#!/usr/bin/env python3
"""Paste-to-insert images for blog posts.

    python3 docs/tools/paste_images.py <post-slug> [--port 8091]

Opens a tiny local page (http://127.0.0.1:8091): paste an image from the clipboard
(or drag & drop files onto it) and it lands in docs/content/blog/<slug>/ — the
matching `![…](file)` markdown line is copied to your clipboard, ready to drop into
index.md. Run `trunk serve` alongside and the post preview hot-reloads as you go.

Stdlib only. If Pillow is installed (`pip install pillow`), images wider than 1600 px
are downscaled and re-encoded to web weight; without it, files are saved as-is.
"""

import argparse
import http.server
import io
import json
import re
import socketserver
import sys
from pathlib import Path

BLOG = Path(__file__).resolve().parent.parent / "content" / "blog"
MAX_WIDTH = 1600

try:
    from PIL import Image
    HAVE_PIL = True
except ImportError:
    HAVE_PIL = False

PAGE = """<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>paste → %(slug)s</title>
<style>
  :root { --parchment:#ece8cf; --panel:#f3f0df; --ink:#24331c; --soft:#4d5b3c;
          --faint:#6b7758; --forest:#3c6b39; --coral:#cf6a2a; --line:rgba(47,82,51,.2); }
  * { box-sizing: border-box; }
  body { margin:0; background:var(--parchment); color:var(--ink);
         font: 15px/1.5 ui-sans-serif, system-ui, sans-serif; padding: 34px 20px; }
  main { max-width: 640px; margin: 0 auto; }
  h1 { font-family: Georgia, serif; font-size: 24px; margin: 0 0 4px; }
  .sub { color: var(--faint); font-family: ui-monospace, monospace; font-size: 12.5px; margin: 0 0 22px; }
  #zone { border: 2px dashed var(--line); border-radius: 14px; background: var(--panel);
          padding: 46px 20px; text-align: center; color: var(--soft); transition: .15s; }
  #zone.hot { border-color: var(--coral); background: #f7f0dd; }
  #zone b { color: var(--forest); }
  ul { list-style: none; padding: 0; margin: 22px 0 0; }
  li { display: flex; align-items: center; gap: 12px; background: var(--panel);
       border: 1px solid var(--line); border-radius: 10px; padding: 8px 12px; margin: 8px 0; }
  li img { width: 52px; height: 40px; object-fit: cover; border-radius: 6px; }
  li code { flex: 1; font: 12px ui-monospace, monospace; color: var(--soft);
            overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  li button { border: 1px solid var(--forest); background: none; color: var(--forest);
              border-radius: 7px; padding: 4px 11px; cursor: pointer; font: 12px ui-monospace, monospace; }
  li button:hover { background: var(--forest); color: var(--parchment); }
  .flash { color: var(--coral); font-weight: 600; }
</style></head><body><main>
<h1>Paste images &rarr; <em>%(slug)s</em></h1>
<p class="sub">saves into docs/content/blog/%(slug)s/ &middot; markdown line auto-copied &middot; %(pil)s</p>
<div id="zone"><b>Paste</b> an image here (Ctrl/Cmd-V) or <b>drop</b> files</div>
<ul id="list"></ul>
<script>
  var zone = document.getElementById('zone'), list = document.getElementById('list');
  function send(file) {
    fetch('/save', { method: 'POST', body: file,
      headers: { 'Content-Type': file.type || 'application/octet-stream',
                 'X-Filename': encodeURIComponent(file.name || '') } })
      .then(function (r) { return r.json(); })
      .then(function (res) {
        var li = document.createElement('li');
        var img = document.createElement('img');
        img.src = '/thumb/' + encodeURIComponent(res.file);
        var code = document.createElement('code');
        code.textContent = res.markdown;
        var btn = document.createElement('button');
        btn.textContent = 'copy';
        btn.onclick = function () {
          navigator.clipboard.writeText(res.markdown);
          btn.textContent = 'copied!'; setTimeout(function(){ btn.textContent = 'copy'; }, 1200);
        };
        li.appendChild(img); li.appendChild(code); li.appendChild(btn);
        list.insertBefore(li, list.firstChild);
        navigator.clipboard.writeText(res.markdown).then(function () {
          zone.innerHTML = '<span class="flash">' + res.file + ' saved &middot; markdown copied ✓</span>';
          setTimeout(reset, 1800);
        }, reset);
      });
  }
  function reset() { zone.innerHTML = '<b>Paste</b> an image here (Ctrl/Cmd-V) or <b>drop</b> files'; }
  window.addEventListener('paste', function (e) {
    for (var i = 0; i < e.clipboardData.items.length; i++) {
      var f = e.clipboardData.items[i].getAsFile && e.clipboardData.items[i].getAsFile();
      if (f && /^image\\//.test(f.type)) send(f);
    }
  });
  ['dragover', 'dragenter'].forEach(function (t) {
    window.addEventListener(t, function (e) { e.preventDefault(); zone.classList.add('hot'); });
  });
  ['dragleave', 'drop'].forEach(function (t) {
    window.addEventListener(t, function (e) { e.preventDefault(); zone.classList.remove('hot'); });
  });
  window.addEventListener('drop', function (e) {
    Array.prototype.forEach.call(e.dataTransfer.files, function (f) {
      if (/^image\\//.test(f.type)) send(f);
    });
  });
</script></main></body></html>
"""

EXT = {"image/png": ".png", "image/jpeg": ".jpg", "image/webp": ".webp", "image/gif": ".gif"}


def next_name(dest, hint, ctype):
    """Pick a filename: slugified original name if free, else img-NN.<ext>."""
    ext = EXT.get(ctype, ".png")
    if hint:
        stem = re.sub(r"[^\w-]+", "-", Path(hint).stem.lower()).strip("-")
        if stem:
            cand = dest / (stem + ext)
            if not cand.exists():
                return cand
    n = 1
    while (dest / ("img-%02d%s" % (n, ext))).exists():
        n += 1
    return dest / ("img-%02d%s" % (n, ext))


def shrink(data, path):
    """Downscale to MAX_WIDTH with Pillow when available; else write as-is."""
    if not HAVE_PIL:
        path.write_bytes(data)
        return
    try:
        img = Image.open(io.BytesIO(data))
    except Exception:
        path.write_bytes(data)
        return
    if img.width > MAX_WIDTH:
        img = img.resize((MAX_WIDTH, round(img.height * MAX_WIDTH / img.width)),
                         Image.LANCZOS)
    save_kw = {"quality": 87} if path.suffix in (".jpg", ".webp") else {"optimize": True}
    img.save(path, **save_kw)


def make_handler(dest, slug):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def _send(self, code, ctype, body):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path.startswith("/thumb/"):
                name = Path(self.path[len("/thumb/"):].replace("%20", " ")).name
                f = dest / name
                if f.exists():
                    self._send(200, "image", f.read_bytes())
                else:
                    self._send(404, "text/plain", b"gone")
                return
            pil = "resizing to %dpx (Pillow)" % MAX_WIDTH if HAVE_PIL \
                else "saved as-is (pip install pillow to auto-resize)"
            self._send(200, "text/html; charset=utf-8",
                       (PAGE % {"slug": slug, "pil": pil}).encode())

        def do_POST(self):
            if self.path != "/save":
                self._send(404, "text/plain", b"nope")
                return
            length = int(self.headers.get("Content-Length", 0))
            data = self.rfile.read(length)
            hint = self.headers.get("X-Filename", "")
            path = next_name(dest, hint, self.headers.get("Content-Type", ""))
            shrink(data, path)
            md = "![](%s)" % path.name
            print("  saved %s (%.0f KB)  ->  %s" % (path.name, path.stat().st_size / 1024, md))
            self._send(200, "application/json",
                       json.dumps({"file": path.name, "markdown": md}).encode())

    return Handler


def main():
    ap = argparse.ArgumentParser(description="Paste/drop images straight into a blog post folder.")
    ap.add_argument("slug", help="post folder name under docs/content/blog/")
    ap.add_argument("--port", type=int, default=8091)
    args = ap.parse_args()

    dest = BLOG / args.slug
    if not (dest / "index.md").exists():
        sys.exit("paste_images: no post at %s — scaffold it first with blog_new.py" % dest)

    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("127.0.0.1", args.port), make_handler(dest, args.slug)) as httpd:
        print("paste images for '%s' at  http://127.0.0.1:%d  (Ctrl-C to stop)" % (args.slug, args.port))
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print()


if __name__ == "__main__":
    main()
