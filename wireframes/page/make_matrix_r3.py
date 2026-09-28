#!/usr/bin/env python3
"""Write page/matrix-r3.html: one row per state, one column per phone direction (from r3-N.html)."""
import pathlib, re

ROOT = pathlib.Path(__file__).resolve().parent.parent
STATES = [("rest", "At rest"), ("search", "Searching, keyboard up"), ("answer", "Answer on the map"),
          ("picker", "A chip picker open"), ("profile", "The profile"), ("days", "The days"),
          ("route", "On the road: a route")]
DIRS = [("1", "P1 · One sheet"), ("2", "P2 · Three views"), ("3", "P3 · Map, strip, tool bar")]

styles, figs = [], {}
for n, _ in DIRS:
    src = (ROOT / f"r3-{n}.html").read_text() if (ROOT / f"r3-{n}.html").exists() else ""
    styles += re.findall(r"<style[^>]*>.*?</style>", src, re.S)
    for m in re.finditer(r'<figure\b[^>]*\bid="(p%s-[\w-]+)"[^>]*>.*?</figure>' % n, src, re.S):
        figs[m[1]] = m[0]

out = ["\n".join(styles), '<div class="mx-scroll"><div class="mx">',
       '<div class="mx-h"></div>' + "".join(f'<div class="mx-h">{t}</div>' for _, t in DIRS)]
for key, label in STATES:
    out.append(f'<div class="mx-s">{label}</div>')
    for n, _ in DIRS:
        out.append(f'<div class="mx-c">{figs.get(f"p{n}-{key}", "<p class=muted>[pending]</p>")}</div>')
out.append("</div></div>")
if "p3-landscape" in figs:
    out.append('<div class="mx-extra">' + figs["p3-landscape"] + "</div>")
(ROOT / "page" / "matrix-r3.html").write_text("\n".join(out))
print("matrix:", len(figs), "figures")
