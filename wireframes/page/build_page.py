#!/usr/bin/env python3
"""Assemble a wireframe page for the Artifact (no html/head/body: the host wraps it).

python3 page/build_page.py out.html [--shell shell-r2.html] [--title "..."] [--local]

In the shell: <!--FRAG:name--> includes ROOT/name.html (a mock fragment), <!--NOTES:x--> includes
page/notes-x.html, <!--INC:name--> includes page/name.html. --local wraps the page in a full html
document for screenshots.
"""
import argparse, pathlib, re

ROOT = pathlib.Path(__file__).resolve().parent.parent
KIT, PAGE = ROOT / "kit", ROOT / "page"


def read(p, default=""):
    p = pathlib.Path(p)
    return p.read_text() if p.exists() else default


def build(shell_name, title):
    shell = read(PAGE / shell_name)
    shell = re.sub(r"<!--FRAG:([\w-]+)-->",
                   lambda m: read(ROOT / f"{m[1]}.html", f'<p class="muted">[{m[1]} pending]</p>'), shell)
    shell = re.sub(r"<!--NOTES:([\w-]+)-->", lambda m: read(PAGE / f"notes-{m[1]}.html"), shell)
    shell = re.sub(r"<!--INC:([\w-]+)-->", lambda m: read(PAGE / f"{m[1]}.html"), shell)
    # round-1 shell placeholders
    shell = shell.replace("<!--COMPARE-->", read(PAGE / "compare.html"))
    shell = shell.replace("<!--MODEL-->", read(PAGE / "model.html"))
    shell = shell.replace("<!--QUESTIONS-->", read(PAGE / "questions.html"))
    css = read(KIT / "base.css") + "\n" + read(PAGE / "page.css")
    head = (
        f"<title>{title}</title>\n"
        '<link rel="preconnect" href="https://fonts.googleapis.com">\n'
        '<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>\n'
        '<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Atkinson+Hyperlegible+Mono:wght@400;600&amp;'
        'family=Atkinson+Hyperlegible+Next:wght@400;500;600;700;800&amp;display=swap">\n'
        f"<style>\n{css}\n</style>\n"
    )
    body = read(KIT / "icons.svg") + "\n" + read(KIT / "maps.svg") + "\n" + shell
    script = f"<script>\n{read(KIT / 'fit.js')}\n</script>\n"
    return head, body, script


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--shell", default="shell.html")
    ap.add_argument("--title", default="Route Planner Wireframes")
    ap.add_argument("--local", action="store_true")
    a = ap.parse_args()
    head, body, script = build(a.shell, a.title)
    if a.local:
        html = f'<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">{head}</head><body>{body}{script}</body></html>'
    else:
        html = head + body + script
    pathlib.Path(a.out).write_text(html)
    print("wrote", a.out, f"{len(html) / 1024:.0f} KB")
