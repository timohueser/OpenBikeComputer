#!/usr/bin/env python3
"""Scaffold a new blog post: `python3 docs/tools/blog_new.py "My post title"`.

Creates docs/content/blog/<slug>/index.md with today's date in the front matter and
prints the authoring loop (trunk serve for live preview, paste_images.py for images).
Stdlib only.
"""

import datetime
import re
import sys
from pathlib import Path

BLOG = Path(__file__).resolve().parent.parent / "content" / "blog"

TEMPLATE = """---
title: {title}
date: {date}
description: One-line teaser shown on the timeline and in the feed.
---

Opening paragraph — this renders slightly larger, like a lede.

## A section

Body text. Standalone images become framed, lightbox-able exhibits:

![Alt text doubles as the lightbox caption](photo.jpg)

<!-- Before/after slider:
```compare
before: old.jpg
after: new.jpg
caption: What changed between the two.
```

Interactive 3D model (convert with docs/tools/step2glb.py):
```model
glb: part.glb
step: part.step
caption: Drag it around.
```
-->
"""


def slugify(title):
    slug = re.sub(r"[^\w\s-]", "", title.lower())
    slug = re.sub(r"[\s_]+", "-", slug).strip("-")
    return slug or "untitled"


def main():
    if len(sys.argv) < 2 or sys.argv[1].startswith("-"):
        sys.exit('usage: python3 docs/tools/blog_new.py "My post title"')
    title = sys.argv[1]
    slug = slugify(title)
    dest = BLOG / slug
    if dest.exists():
        sys.exit("blog: %s already exists" % dest)
    dest.mkdir(parents=True)
    (dest / "index.md").write_text(
        TEMPLATE.format(title=title, date=datetime.date.today().isoformat())
    )
    print("created %s" % (dest / "index.md"))
    print()
    print("next steps:")
    print("  live preview   trunk serve --config docs/Trunk.toml   (hot-reloads on save)")
    print("  paste images   python3 docs/tools/paste_images.py %s" % slug)
    print("  3D model       python3 docs/tools/step2glb.py part.step %s/part.glb" % dest)


if __name__ == "__main__":
    main()
