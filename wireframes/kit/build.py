#!/usr/bin/env python3
"""Assemble a local preview page from fragments: python3 kit/build.py out.html frag1.html [frag2.html ...]"""
import pathlib, sys

KIT = pathlib.Path(__file__).parent

def page(fragments, title="Route planner wireframes"):
    css = (KIT / "base.css").read_text()
    icons = (KIT / "icons.svg").read_text()
    maps = (KIT / "maps.svg").read_text() if (KIT / "maps.svg").exists() else ""
    fit = (KIT / "fit.js").read_text()
    body = "\n".join(pathlib.Path(f).read_text() for f in fragments)
    return f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Atkinson+Hyperlegible+Mono:wght@400;600&family=Atkinson+Hyperlegible+Next:wght@400;500;600;700;800&display=swap">
<style>{css}
body{{margin:0;background:var(--parchment);color:var(--ink);font-family:var(--sans);padding:24px}}</style>
</head><body>
{icons}
{maps}
{body}
<script>{fit}</script>
</body></html>"""

if __name__ == "__main__":
    pathlib.Path(sys.argv[1]).write_text(page(sys.argv[2:]))
    print("wrote", sys.argv[1])
