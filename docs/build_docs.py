#!/usr/bin/env python3
"""OpenBikeComputer documentation renderer.

Markdown -> themed static HTML, with a left-sidebar layout and an on-page table of
contents. Stdlib only (no pip dependencies), Python 3.9+ — so the site build never
breaks on a missing package and a Trunk `pre_build` hook can run it anywhere.

Author docs in `content/*.md`; the nav/order lives in `content/nav.json`; the shared
shell is `templates/page.html` and the theme is `assets/docs.css`. Output goes to
`docs/` (the dir Trunk copy-dirs into `dist/docs/`).

The blog ("expedition log") rides the same renderer: one folder per post under
`content/blog/<slug>/` holding an `index.md` (front matter: `title`, `date`,
`description`) plus its images/models, which are copied verbatim next to the rendered
page. Output goes to `blog/` (a second Trunk copy-dir), with a timeline index and an
Atom feed at `blog/feed.xml`. Two extra fenced directives are available (blog or docs):
```` ```compare ```` (before/after image slider) and ```` ```model ```` (interactive
GLB viewer + optional STEP download) — each takes `key: value` lines; see BLOG.md.

Supported markdown (a small, predictable CommonMark-ish subset — see DOCS_PLAN.md):
headings (auto-slugged), paragraphs, `**bold**`, `*italic*`, `` `code` ``,
`[text](href)`, `-`/`1.` lists (incl. one level of nesting), ``` fenced code,
pipe tables, `>` blockquote callouts, `---` rules, and **raw block HTML/SVG
passthrough** (a line starting with a block tag at column 0 is emitted verbatim — this
is how the inline SVG figures embed). A `src:` link scheme expands to a repo file link.
HTML comment blocks (a line starting with `<!--`, through the line carrying `-->`) are
authoring notes: stripped from the output. The shared topo/header shell lives in
`templates/_sitehead.html` and is injected into every page as `{{site_head}}`.

Run directly (`python3 docs/build_docs.py`) or let the Trunk hook run it. Pass
`--check-links` to additionally verify every internal anchor link resolves to a real
page and heading id (the cross-page `#anchor` audit CI runs) and exit non-zero if not.
"""

import datetime
import html
import json
import os
import re
import shutil
import sys
from pathlib import Path
from urllib.parse import urljoin

ROOT = Path(__file__).resolve().parent          # docs/
CONTENT = ROOT / "content"
TEMPLATE = ROOT / "templates" / "page.html"
SITEHEAD_TEMPLATE = ROOT / "templates" / "_sitehead.html"   # shared topo + header
ASSETS = ROOT / "assets"
OUT = ROOT / "docs"                              # generated; Trunk copy-dirs this

# The docs pages have a slide-out sidebar on mobile; the toggle only ships there
# (blog pages have no sidebar, so the partial gets an empty {{nav_toggle}}).
NAV_TOGGLE = (
    '<button class="nav-toggle" aria-label="Toggle navigation" aria-expanded="false">'
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" '
    'stroke-linecap="round"><path d="M3 6h18M3 12h18M3 18h18"/></svg></button>'
)

BLOG_CONTENT = CONTENT / "blog"                  # one folder per post
BLOG_OUT = ROOT / "blog"                         # generated; second Trunk copy-dir
BLOG_POST_TEMPLATE = ROOT / "templates" / "blog_post.html"
BLOG_INDEX_TEMPLATE = ROOT / "templates" / "blog_index.html"

REPO = "https://github.com/timohueser/OpenBikeComputer"
BRANCH = "main"
SITE = "https://timohueser.github.io/OpenBikeComputer/"   # absolute base for the feed

MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
          "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]

# Tags whose appearance at column 0 begins a raw HTML/SVG block we pass through verbatim.
RAW_BLOCK_TAGS = {
    "svg", "div", "figure", "figcaption", "table", "thead", "tbody", "tr", "td", "th",
    "details", "summary", "section", "aside", "nav", "header", "footer", "style",
    "script", "pre", "blockquote", "ul", "ol", "canvas", "video", "iframe", "picture",
}
VOID_TAGS = {"img", "hr", "br", "input", "meta", "link", "col", "area", "base",
             "embed", "source", "track", "wbr"}


# --------------------------------------------------------------------------- inline

def esc(s):
    return html.escape(s, quote=False)


def slugify(text):
    t = re.sub(r"<[^>]+>", "", text).lower()
    t = re.sub(r"[^\w\s-]", "", t)
    t = re.sub(r"[\s_]+", "-", t).strip("-")
    return t or "section"


CODE_RE = re.compile(r"`([^`]+)`")
IMG_RE = re.compile(r"!\[([^\]]*)\]\(([^)\s]+)\)")
LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)\s]+)(?:\s+\"([^\"]*)\")?\)")


def inline_lite(text):
    """Bold + italic only (for nested contexts like link labels)."""
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(r"\*(.+?)\*", r"<em>\1</em>", text)
    return text


def inline(text):
    """Full inline pass. Code spans and links are stashed behind placeholders so the
    bold/italic pass can't reach inside them, then restored (iteratively, for nesting)."""
    store = []

    def stash(frag):
        store.append(frag)
        return "\x00%d\x00" % (len(store) - 1)

    text = IMG_RE.sub(
        lambda m: stash('<img src="%s" alt="%s" loading="lazy">'
                        % (esc(m.group(2)), esc(m.group(1)))),
        text,
    )
    text = CODE_RE.sub(lambda m: stash("<code>%s</code>" % esc(m.group(1))), text)

    def link_sub(m):
        label, href = m.group(1), m.group(2)
        cls = ""
        if href.startswith("src:"):
            href = "%s/blob/%s/%s" % (REPO, BRANCH, href[4:])
            cls = ' class="srclink"'
        ext = ' target="_blank" rel="noopener"' if href.startswith("http") else ""
        return stash('<a%s href="%s"%s>%s</a>' % (cls, esc(href), ext, inline_lite(label)))

    text = LINK_RE.sub(link_sub, text)
    text = inline_lite(text)

    for _ in range(6):
        if "\x00" not in text:
            break
        text = re.sub(r"\x00(\d+)\x00", lambda m: store[int(m.group(1))], text)
    return text


# ----------------------------------------------------------------- fenced directives

def attr(s):
    """Escape for use inside a double-quoted HTML attribute."""
    return html.escape(s, quote=True)


def parse_kv(buf):
    """`key: value` lines (the body of a ```compare / ```model fence) -> dict."""
    kv = {}
    for ln in buf:
        if ":" in ln:
            k, v = ln.split(":", 1)
            kv[k.strip().lower()] = v.strip()
    return kv


def render_directive(lang, buf):
    """Expand a ```compare or ```model fence into its figure markup.

    The markup is deliberately minimal + functional without JS (compare: the two images
    stacked; model: the download link). assets/blog.js and assets/glb-viewer.js enhance
    them into the draggable slider and the orbit viewer.
    """
    kv = parse_kv(buf)
    cap = "<figcaption>%s</figcaption>" % inline(kv["caption"]) if kv.get("caption") else ""
    if lang == "compare":
        before, after = kv.get("before", ""), kv.get("after", "")
        lb = kv.get("label-before", "Before")
        la = kv.get("label-after", "After")
        return (
            '<figure class="fig cmp" data-label-before="%s" data-label-after="%s">'
            '<div class="cmp-stage"><img src="%s" alt="%s" loading="lazy">'
            '<img src="%s" alt="%s" loading="lazy"></div>%s</figure>'
            % (attr(lb), attr(la), attr(before), attr(lb), attr(after), attr(la), cap)
        )
    dl = ""
    if kv.get("step"):
        dl = '<a class="model-dl" href="%s" download>Download STEP</a>' % attr(kv["step"])
    return (
        '<figure class="fig model" data-glb="%s">'
        '<div class="model-stage" tabindex="0" role="img" aria-label="%s"></div>'
        '<div class="model-bar"><span class="model-hint">drag to orbit &#183; click + scroll to zoom'
        " &#183; double-click to reset</span>%s</div>%s</figure>"
        % (attr(kv.get("glb", "")), attr(kv.get("caption", "Interactive 3D model")), dl, cap)
    )


# --------------------------------------------------------------------------- blocks

HEADING_RE = re.compile(r"(#{1,6})\s+(.*)")
LIST_RE = re.compile(r"^(\s*)([-*+]|\d+\.)\s+(.*)$")
FENCE_RE = re.compile(r"```+\s*([\w-]*)\s*$")
HR_RE = re.compile(r"(---|\*\*\*|___)\s*$")


def raw_tag(line):
    m = re.match(r"<\s*(/?)([a-zA-Z][\w-]*)", line.strip())
    return m and m.group(2).lower() in (RAW_BLOCK_TAGS | VOID_TAGS)


def consume_raw(lines, i):
    """Emit a raw HTML/SVG block verbatim. For container tags, read until the matching
    close (counting opens/closes of the *same* tag so nested groups are fine)."""
    line = lines[i]
    m = re.match(r"<\s*([a-zA-Z][\w-]*)", line.strip())
    tag = m.group(1).lower()
    if tag in VOID_TAGS or re.search(r"/>\s*$", line):
        return line, i + 1
    depth = 0
    buf = []
    n = len(lines)
    while i < n:
        cur = lines[i]
        depth += len(re.findall(r"<%s\b" % tag, cur, re.I))
        depth -= len(re.findall(r"</%s>" % tag, cur, re.I))
        buf.append(cur)
        i += 1
        if depth <= 0:
            break
    return "\n".join(buf), i


def split_row(line):
    line = line.strip().strip("|")
    return [c.strip() for c in re.split(r"(?<!\\)\|", line)]


def consume_table(lines, i):
    header = split_row(lines[i])
    seps = split_row(lines[i + 1])
    aligns = []
    for s in seps:
        left, right = s.startswith(":"), s.endswith(":")
        aligns.append("center" if left and right else "right" if right else "left" if left else "")
    i += 2
    n = len(lines)
    rows = []
    while i < n and "|" in lines[i] and lines[i].strip():
        rows.append(split_row(lines[i]))
        i += 1

    def cell(tag, text, idx):
        a = aligns[idx] if idx < len(aligns) else ""
        style = ' style="text-align:%s"' % a if a else ""
        return "<%s%s>%s</%s>" % (tag, style, inline(text), tag)

    head = "".join(cell("th", c, j) for j, c in enumerate(header))
    body = "".join("<tr>%s</tr>" % "".join(cell("td", c, j) for j, c in enumerate(r)) for r in rows)
    return '<div class="table-wrap"><table><thead><tr>%s</tr></thead><tbody>%s</tbody></table></div>' % (head, body), i


def parse_list(lines, i, indent):
    n = len(lines)
    first = LIST_RE.match(lines[i])
    ordered = first.group(2).endswith(".")
    tag = "ol" if ordered else "ul"
    items = []
    while i < n:
        line = lines[i]
        if not line.strip():
            j = i + 1
            while j < n and not lines[j].strip():
                j += 1
            nxt = LIST_RE.match(lines[j]) if j < n else None
            if nxt and len(nxt.group(1)) >= indent:
                i = j
                continue
            break
        m = LIST_RE.match(line)
        if not m:
            break
        cur_indent = len(m.group(1))
        if cur_indent < indent:
            break
        if cur_indent > indent:
            sub, i = parse_list(lines, i, cur_indent)
            if items:
                items[-1] += sub
            continue
        items.append("<li>%s" % inline(m.group(3).strip()))
        i += 1
    lis = "".join(it + "</li>" for it in items)
    return "<%s>%s</%s>" % (tag, lis, tag), i


def is_block_start(line, nxt):
    s = line.lstrip()
    if not s:
        return True
    if HEADING_RE.match(line) or HR_RE.match(line) or FENCE_RE.match(line):
        return True
    if LIST_RE.match(line) or s.startswith(">"):
        return True
    if raw_tag(line):
        return True
    if "|" in line and re.match(r"^\s*\|?[\s:|-]*-[\s:|-]*\|?\s*$", nxt) and "-" in nxt:
        return True
    return False


def render_blocks(md):
    lines = md.split("\n")
    out = []
    toc = []
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        if not line.strip():
            i += 1
            continue
        if line.lstrip().startswith("<!--"):
            # HTML comments are authoring notes — dropped from the output entirely
            # (the blog_new.py scaffold leans on this to keep its examples unpublished).
            while i < n and "-->" not in lines[i]:
                i += 1
            i += 1
            continue
        if raw_tag(line):
            frag, i = consume_raw(lines, i)
            out.append(frag)
            continue
        m = HEADING_RE.match(line)
        if m:
            level = len(m.group(1))
            text = m.group(2).strip()
            slug = slugify(text)
            inner = inline(text)
            if level in (2, 3):
                toc.append((level, slug, re.sub(r"<[^>]+>", "", inner)))
            out.append('<h%d id="%s">%s<a class="anchor" href="#%s" aria-hidden="true">#</a></h%d>'
                       % (level, slug, inner, slug, level))
            i += 1
            continue
        if HR_RE.match(line):
            out.append("<hr>")
            i += 1
            continue
        m = FENCE_RE.match(line)
        if m:
            lang = m.group(1)
            i += 1
            buf = []
            while i < n and not re.match(r"```+\s*$", lines[i]):
                buf.append(lines[i])
                i += 1
            i += 1
            if lang in ("compare", "model"):
                out.append(render_directive(lang, buf))
                continue
            cls = ' class="lang-%s"' % lang if lang else ""
            out.append('<pre class="code"><code%s>%s</code></pre>' % (cls, esc("\n".join(buf))))
            continue
        if line.startswith(">"):
            buf = []
            while i < n and lines[i].startswith(">"):
                buf.append(re.sub(r"^>\s?", "", lines[i]))
                i += 1
            inner, _ = render_blocks("\n".join(buf))
            out.append('<blockquote class="callout">%s</blockquote>' % inner)
            continue
        if "|" in line and i + 1 < n and re.match(r"^\s*\|?[\s:|-]*-[\s:|-]*\|?\s*$", lines[i + 1]) and "-" in lines[i + 1]:
            frag, i = consume_table(lines, i)
            out.append(frag)
            continue
        if LIST_RE.match(line):
            indent = len(LIST_RE.match(line).group(1))
            frag, i = parse_list(lines, i, indent)
            out.append(frag)
            continue
        buf = []
        while i < n and lines[i].strip() and not is_block_start(lines[i], lines[i + 1] if i + 1 < n else ""):
            buf.append(lines[i].strip())
            i += 1
        out.append("<p>%s</p>" % inline(" ".join(buf)))
    return "\n".join(out), toc


# ----------------------------------------------------------------------------- page

def split_front_matter(text):
    if text.startswith("---"):
        m = re.match(r"^---\s*\n(.*?)\n---\s*\n", text, re.S)
        if m:
            fm = {}
            for ln in m.group(1).split("\n"):
                if ":" in ln:
                    k, v = ln.split(":", 1)
                    fm[k.strip()] = v.strip().strip('"')
            return fm, text[m.end():]
    return {}, text


def page_url(path):
    """Source path (no extension) -> root-relative URL. 'index'->'', 'a/index'->'a/',
    'a/b'->'a/b/'."""
    if path == "index":
        return ""
    if path.endswith("/index"):
        path = path[:-len("/index")]
    return path + "/"


def base_for(url):
    depth = len([s for s in url.split("/") if s])
    return "../" * depth


def render_nav(nav, current_url, base):
    parts = ['<nav class="sidebar-nav" aria-label="Documentation">']
    for sec in nav["sections"]:
        if sec.get("title"):
            parts.append('<div class="nav-section">%s</div>' % esc(sec["title"]))
        parts.append('<ul class="nav-list">')
        for pg in sec["pages"]:
            u = page_url(pg["path"])
            href = base + u
            if href == "":
                href = "./"
            active = ' class="active"' if u == current_url else ""
            badge = ' <span class="soon">soon</span>' if pg.get("soon") else ""
            parts.append('<li><a%s href="%s">%s%s</a></li>' % (active, href, esc(pg["title"]), badge))
        parts.append("</ul>")
    parts.append("</nav>")
    return "\n".join(parts)


def render_toc(toc):
    if len(toc) < 3:
        return ""
    items = "".join('<li class="lvl%d"><a href="#%s">%s</a></li>' % (lvl, slug, esc(text))
                    for lvl, slug, text in toc)
    return '<aside class="toc"><div class="toc-title">On this page</div><ul>%s</ul></aside>' % items


def nav_title_for(nav, path):
    for sec in nav["sections"]:
        for pg in sec["pages"]:
            if pg["path"] == path:
                return pg["title"]
    return path


# ------------------------------------------------------------------------------ blog

def human_date(d):
    """date -> '12 Jul 2026' (locale-independent)."""
    return "%d %s %d" % (d.day, MONTHS[d.month - 1], d.year)


def read_minutes(md):
    """Word count / 220 wpm, floor 1 — shown as 'n min read' on the post."""
    return max(1, round(len(re.findall(r"\w+", md)) / 220))


def load_posts():
    """Read every content/blog/<slug>/index.md, newest first. Front matter must carry
    `title:` and an ISO `date:`; `description:` feeds the timeline teaser + the feed."""
    posts = []
    if not BLOG_CONTENT.exists():
        return posts
    for src in sorted(BLOG_CONTENT.glob("*/index.md")):
        fm, body = split_front_matter(src.read_text())
        slug = src.parent.name
        if not fm.get("title") or not fm.get("date"):
            sys.exit("blog: %s needs 'title:' and 'date:' front matter" % src)
        try:
            date = datetime.date.fromisoformat(fm["date"])
        except ValueError:
            sys.exit("blog: %s has a bad date %r (want YYYY-MM-DD)" % (src, fm["date"]))
        posts.append({
            "slug": slug, "dir": src.parent, "title": fm["title"], "date": date,
            "description": fm.get("description", ""), "body": body,
            "minutes": read_minutes(body),
        })
    posts.sort(key=lambda p: (p["date"].isoformat(), p["slug"]), reverse=True)
    return posts


def render_timeline(posts):
    """The expedition-log index: month-grouped entries hanging off a waypoint rail."""
    parts = []
    group = None
    for p in posts:
        key = (p["date"].year, p["date"].month)
        if key != group:
            if group is not None:
                parts.append("</ol></section>")
            parts.append('<section class="tl-group"><div class="tl-label">%s %d</div>'
                         '<ol class="tl-entries">' % (MONTHS[key[1] - 1], key[0]))
            group = key
        teaser = "<p class='tl-teaser'>%s</p>" % inline(p["description"]) if p["description"] else ""
        parts.append(
            '<li class="tl-entry"><a class="tl-link" href="%s/">'
            '<span class="tl-meta"><time datetime="%s">%s</time> &#183; %d min</span>'
            "<h2>%s</h2>%s</a></li>"
            % (attr(p["slug"]), p["date"].isoformat(), human_date(p["date"]),
               p["minutes"], esc(p["title"]), teaser)
        )
    if group is not None:
        parts.append("</ol></section>")
    if not parts:
        return '<p class="tl-empty">No entries yet — the trail starts here.</p>'
    return '<div class="timeline">%s</div>' % "\n".join(parts)


def absolutize(content, base):
    """Rewrite src/href attributes to absolute URLs for feed readers. urljoin both
    normalizes relative paths ('../feed.xml' loses its dot-segments) and anchors bare
    `#fragment` links to the post URL — a feed reader has no page to resolve them
    against. The `#` heading anchors themselves are stripped first: they'd render as
    stray '#' glyphs outside our CSS."""
    content = re.sub(r'<a class="anchor"[^>]*>#</a>', "", content)
    return re.sub(
        r'(src|href)="([^"]+)"',
        lambda m: '%s="%s"' % (m.group(1), urljoin(base, m.group(2))),
        content,
    )


def write_feed(posts, rendered_posts):
    """Atom feed at blog/feed.xml — full content inline, URLs absolutized."""
    def x(s):
        return html.escape(s, quote=True)

    updated = max(p["date"] for p in posts).isoformat() if posts else "1970-01-01"
    entries = []
    for p in posts:
        url = SITE + "blog/" + p["slug"] + "/"
        content = absolutize(rendered_posts[p["slug"]], url)
        entries.append(
            "<entry><title>%s</title><link href=\"%s\"/><id>%s</id>"
            "<updated>%sT00:00:00Z</updated><summary>%s</summary>"
            '<content type="html">%s</content></entry>'
            % (x(p["title"]), x(url), x(url), p["date"].isoformat(),
               x(p["description"]), x(content))
        )
    feed = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<feed xmlns="http://www.w3.org/2005/Atom">\n'
        "<title>OpenBikeComputer — expedition log</title>\n"
        '<link href="%sblog/"/><link rel="self" href="%sblog/feed.xml"/>\n'
        "<id>%sblog/</id><updated>%sT00:00:00Z</updated>\n"
        "<author><name>Timo Hüser</name></author>\n%s\n</feed>\n"
        % (SITE, SITE, SITE, updated, "\n".join(entries))
    )
    (BLOG_OUT / "feed.xml").write_text(feed)


def prevnext_html(posts, idx):
    """Older/newer links under a post (list is newest-first: idx+1 is older).
    Returns the whole <nav>, or '' for a lone post — no point shipping an empty grid."""
    cells = []
    if idx + 1 < len(posts):
        o = posts[idx + 1]
        cells.append('<a class="pn older" href="../%s/"><span>&#8592; Older</span>%s</a>'
                     % (attr(o["slug"]), esc(o["title"])))
    else:
        cells.append("<span></span>")
    if idx > 0:
        nw = posts[idx - 1]
        cells.append('<a class="pn newer" href="../%s/"><span>Newer &#8594;</span>%s</a>'
                     % (attr(nw["slug"]), esc(nw["title"])))
    if len(posts) < 2:
        return ""
    return '<nav class="post-pn" aria-label="Adjacent entries">%s</nav>' % "".join(cells)


def fill(template, repl):
    for k, v in repl.items():
        template = template.replace("{{%s}}" % k, v)
    return template


def builder_link(site_root):
    """The header's link to the static map builder, or '' when there isn't one.

    The builder is a sibling app in the same published artifact (`/builder/`), not a
    rendered page — the site deploy sets OBC_BUILDER_PATH to its site-root-relative
    path *only* when that deployment has a map catalog configured. Until the bakery
    publishes one, the builder opens on "couldn't load the map catalog", and a nav
    link straight into that is a broken front door. So the deploy decides, and a local
    `python3 build_docs.py` (no variable set) simply renders the header without it."""
    path = os.environ.get("OBC_BUILDER_PATH", "").strip()
    if not path:
        return ""
    return '<a href="%s%s">Map builder</a>' % (esc(site_root), esc(path))


def site_head(site_root, crumb, nav_toggle=""):
    """Expand the shared topo + header partial (templates/_sitehead.html) — one copy
    of the markup, so a nav change can't drift between the docs and blog shells."""
    return fill(SITEHEAD_TEMPLATE.read_text(),
                {"site_root": site_root, "crumb": crumb, "nav_toggle": nav_toggle,
                 "builder_link": builder_link(site_root)})


def build_blog(rendered):
    """Render blog/ (posts + timeline index + feed) and register pages for the link
    check under site-root-relative 'blog/…' keys."""
    posts = load_posts()
    post_tpl = BLOG_POST_TEMPLATE.read_text()
    index_tpl = BLOG_INDEX_TEMPLATE.read_text()

    if BLOG_OUT.exists():
        shutil.rmtree(BLOG_OUT)
    BLOG_OUT.mkdir(parents=True)
    (BLOG_OUT / ".gitkeep").touch()   # same trunk-watch reason as docs/ (see main())
    shutil.copytree(ASSETS, BLOG_OUT / "assets")

    rendered_posts = {}
    for idx, p in enumerate(posts):
        content, _toc = render_blocks(p["body"])
        dest = BLOG_OUT / p["slug"]
        dest.mkdir(parents=True)
        for f in p["dir"].iterdir():          # images / .glb / .step live next to the md
            if f.name == "index.md" or f.name.startswith("."):
                continue
            if f.is_dir():
                shutil.copytree(f, dest / f.name)
            else:
                shutil.copy2(f, dest / f.name)
        page = fill(post_tpl, {
            "title": esc(p["title"]),
            # attr(): these land in content="…" attributes — esc() leaves `"` alone.
            "description": attr(p["description"] or "OpenBikeComputer expedition log."),
            "site_head": site_head("../../", "/ log"),
            "base": "../",
            "site_root": "../../",
            "css": "../assets/docs.css",
            "blog_css": "../assets/blog.css",
            "date_iso": p["date"].isoformat(),
            "date_human": human_date(p["date"]),
            "minutes": str(p["minutes"]),
            "content": content,
            "prevnext": prevnext_html(posts, idx),
        })
        (dest / "index.html").write_text(page)
        rendered_posts[p["slug"]] = content
        rendered["blog/%s/" % p["slug"]] = content
        print("  blog/%s/index.md -> blog/%s/index.html" % (p["slug"], p["slug"]))

    timeline = render_timeline(posts)
    index_page = fill(index_tpl, {
        "title": "Expedition log",
        "description": "Build notes and field reports from the OpenBikeComputer workbench.",
        "site_head": site_head("../", "/ log"),
        "base": "",
        "site_root": "../",
        "css": "assets/docs.css",
        "blog_css": "assets/blog.css",
        "content": timeline,
    })
    (BLOG_OUT / "index.html").write_text(index_page)
    rendered["blog/"] = timeline
    write_feed(posts, rendered_posts)
    print("blog: rendered %d post(s) + index + feed into %s" % (len(posts), BLOG_OUT.relative_to(ROOT)))


# ----------------------------------------------------------------------- link check

# Heading ids and anchor hrefs straight out of the *rendered* HTML — so the check
# validates exactly the ids/links that ship, not a re-derivation that could drift.
HEADING_ID_RE = re.compile(r'<h[1-6]\b[^>]*\bid="([^"]+)"')
ANCHOR_HREF_RE = re.compile(r'<a\b[^>]*\bhref="([^"]+)"')


def check_links(rendered):
    """Verify every internal anchor link resolves to a real page and (if it carries a
    `#fragment`) a real heading id on that page. `rendered` maps each page's
    site-root-relative URL ('docs/' for the docs index, 'blog/…' for posts) to its
    content HTML, so cross-tree links (docs <-> blog) validate too. Returns the number
    of broken links — the cross-page `../page/#anchor` check CLAUDE.md otherwise asks
    me to do by hand."""
    pages = set(rendered)
    slugs = {url: set(HEADING_ID_RE.findall(content)) for url, content in rendered.items()}

    broken = []
    for url, content in sorted(rendered.items()):
        for href in ANCHOR_HREF_RE.findall(content):
            # Only internal links: external (http/mailto, incl. expanded `src:` links) and
            # protocol-relative URLs resolve elsewhere and aren't ours to validate.
            # feed.xml isn't a page; skip it.
            if re.match(r"[a-z]+:", href) or href.startswith("//") or href.endswith("feed.xml"):
                continue
            path, _, frag = href.partition("#")
            target = urljoin(url, path).lstrip("/") if path else url
            # The map builder is a sibling *app* in the published artifact, not a page
            # this renderer produces — there is no HTML here to validate it against.
            if target.startswith("builder/"):
                continue
            label = "/" + url
            if target not in pages:
                broken.append("%s: '%s' -> no such page '%s'" % (label, href, target or "(index)"))
            elif frag and frag not in slugs[target]:
                broken.append("%s: '%s' -> no '#%s' heading on '%s'" % (label, href, frag, target or "(index)"))

    if broken:
        print("docs: %d broken internal link(s):" % len(broken), file=sys.stderr)
        for b in broken:
            print("  ✗ %s" % b, file=sys.stderr)
    else:
        print("docs: all internal anchor links resolve")
    return len(broken)


def main():
    check = "--check-links" in sys.argv[1:]
    if not TEMPLATE.exists():
        sys.exit("missing template: %s" % TEMPLATE)
    nav = json.loads((CONTENT / "nav.json").read_text())
    template = TEMPLATE.read_text()

    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    # The committed placeholder keeping OUT present on fresh checkouts (trunk
    # canonicalizes its [watch] ignore path at startup, before this hook runs, #625).
    # Recreate it so a build doesn't delete the tracked file.
    (OUT / ".gitkeep").touch()
    shutil.copytree(ASSETS, OUT / "assets")

    rendered = {}
    pages = [pg["path"] for sec in nav["sections"] for pg in sec["pages"]]
    for path in pages:
        src = CONTENT / (path + ".md")
        if not src.exists():
            print("  ! skipping missing %s" % src, file=sys.stderr)
            continue
        fm, body = split_front_matter(src.read_text())
        content, toc = render_blocks(body)
        url = page_url(path)
        base = base_for(url)
        title = fm.get("title") or nav_title_for(nav, path)
        desc = fm.get("description", "Documentation for OpenBikeComputer.")

        out_html = fill(template, {
            "title": esc(title),
            "description": attr(desc),   # lands in content="…"; esc() leaves `"` alone
            "site_head": site_head(base + "../", "/ docs", NAV_TOGGLE),
            "base": base,
            "site_root": base + "../",
            "css": base + "assets/docs.css",
            "nav": render_nav(nav, url, base),
            "toc": render_toc(toc),
            "content": content,
            "has_toc": "has-toc" if render_toc(toc) else "",
        })

        dest = OUT / "index.html" if url == "" else OUT / url / "index.html"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(out_html)
        rendered["docs/" + url] = content
        print("  %s -> %s" % (path + ".md", dest.relative_to(ROOT)))

    print("docs: rendered %d pages into %s" % (len(pages), OUT.relative_to(ROOT)))

    build_blog(rendered)

    if check and check_links(rendered):
        sys.exit(1)


if __name__ == "__main__":
    main()
