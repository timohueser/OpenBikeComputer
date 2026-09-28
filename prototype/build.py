#!/usr/bin/env python3
"""Inline src/ and the wireframe kit's icons and maps into one self-contained index.html: python3 build.py"""
import pathlib, re

HERE = pathlib.Path(__file__).parent
KIT = HERE.parent / "wireframes" / "kit"
SRC = HERE / "src"
JS = ["data.js", "line.js", "parser.js", "map.js", "sheet.js", "answers.js", "trip.js", "route.js", "profile.js", "box.js", "view.js", "scene.js", "app.js"]

maps = (KIT / "maps.svg").read_text()
# Map labels keep their pixel size at any zoom: their font size becomes --fs, and app.css scales it by --lk.
maps = re.sub(r"font-size:(\d+(?:\.\d+)?)px", r"--fs:\1px", maps)

page = (SRC / "page.html").read_text()
page = page.replace("{{app.css}}", (SRC / "app.css").read_text())
page = page.replace("{{icons.svg}}", (KIT / "icons.svg").read_text())
page = page.replace("{{maps.svg}}", maps)
page = page.replace("{{js}}", "\n".join((SRC / f).read_text() for f in JS))
(HERE / "index.html").write_text(page)
print("wrote", HERE / "index.html", len(page), "bytes")
