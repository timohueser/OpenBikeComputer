"""The compact fact format of specs/wikimedia-content.md, shared by API and dump acquisition."""
from __future__ import annotations

from html import escape
from html.parser import HTMLParser
import json

try:
    from ..landmark_capture import LANGUAGES, PHOTO_FALLBACK, api, batches
except ImportError:
    from landmark_capture import LANGUAGES, PHOTO_FALLBACK, api, batches

CLAIMS = {"P31", "P279", "P625", "P17", "P131", "P37", "P18", "P373", "P4291", "P8592", "P5252"}
VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"}


def compact_entity(value: dict) -> dict:
    compact = {field: value[field] for field in ("id", "lastrevid", "labels", "redirects") if field in value}
    compact["claims"] = {prop: [{field: claim[field] for field in ("mainsnak", "rank", "qualifiers") if field in claim} for claim in claims]
                         for prop, claims in value.get("claims", {}).items() if prop in CLAIMS}
    compact["sitelinks"] = {key: {"title": link["title"]} for key, link in value.get("sitelinks", {}).items()
                            if key in {f"{language}wiki" for language in LANGUAGES}}
    compact["labels"] = {language: label for language, label in value.get("labels", {}).items() if language in LANGUAGES}
    return compact


def encoded(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()


class Lead(HTMLParser):
    """Retain Parsoid's lead section and original page notices with their markup."""

    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.stack = []
        self.capture_depth = None
        self.parts = []
        self.found = False
        self.finished = False

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if not self.found and ((tag == "section" and attrs.get("data-mw-section-id") == "0")
                               or (tag == "div" and "mw-parser-output" in attrs.get("class", "").split())):
            self.capture_depth = len(self.stack)
            self.found = True
        if self.capture_depth is not None and tag == "h2":
            self.parts.extend(f"</{open_tag}>" for open_tag in reversed(self.stack[self.capture_depth:]))
            self.capture_depth = None
            self.finished = True
        if self.capture_depth is not None:
            compact = "".join(f' {key}="{escape(value or "", quote=True)}"' for key, value in attrs.items()
                              if key in {"href", "src", "class", "id", "alt", "title", "data-mw-section-id", "typeof", "resource"})
            self.parts.append(f"<{tag}{compact}>")
        if tag not in VOID:
            self.stack.append(tag)

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        if tag not in VOID:
            self.handle_endtag(tag)

    def handle_endtag(self, tag):
        if tag not in self.stack:
            return
        depth = len(self.stack) - 1 - self.stack[::-1].index(tag)
        if self.capture_depth is not None:
            self.parts.append(f"</{tag}>")
            if depth == self.capture_depth:
                self.capture_depth = None
                self.finished = True
        del self.stack[depth:]

    def handle_data(self, data):
        if self.capture_depth is not None:
            self.parts.append(data)

    def handle_entityref(self, name):
        self.handle_data(f"&{name};")

    def handle_charref(self, name):
        self.handle_data(f"&#{name};")


def lead_html(html: str) -> str:
    parser = Lead()
    parser.feed(html)
    if not parser.found or not parser.finished:
        raise ValueError("article has no complete Parsoid lead section")
    return "".join(parser.parts)


class Notices(Lead):
    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        classes = attrs.get("class", "").split()
        if self.capture_depth is None and (attrs.get("id") == "footer-info-copyright" or any(name in classes for name in ("licensetpl", "attribution", "source-attribution"))):
            self.capture_depth = len(self.stack)
        if self.capture_depth is not None:
            self.parts.append(self.get_starttag_text())
        if tag not in VOID:
            self.stack.append(tag)


def original_notices(html: str) -> str:
    parser = Notices()
    parser.feed(html)
    return "".join(parser.parts)


def resolved_pages(raw: dict, requested: list[str]) -> list[tuple[str, dict | None, list[dict]]]:
    query = raw.get("query", {})
    pages = query.get("pages", [])
    if isinstance(pages, dict):
        pages = list(pages.values())
    by_title = {page["title"]: page for page in pages}
    links = query.get("normalized", []) + query.get("redirects", [])
    edges = {link["from"]: link for link in links}
    result = []
    for requested_title in requested:
        title, proof, seen = requested_title, [], set()
        while title in edges:
            if title in seen:
                raise ValueError("cyclic title redirects")
            seen.add(title)
            edge = edges[title]
            proof.append(edge)
            title = edge["to"]
        result.append((requested_title, by_title.get(title), proof))
    return result


def commons_metadata(fetch, titles: list[str]):
    """Yield each normalised `File:` title with its `commons` fact, or with the reason it has none.
    `fetch(url)` returns the API response or `None`. Category pagination must finish."""
    for chunk in batches(titles):
        params = dict(action="query", formatversion=2, titles="|".join(chunk), redirects=1, prop="imageinfo|revisions|categories", rvprop="ids|timestamp", iiprop="url|timestamp|sha1|extmetadata|mime|size", iilimit=1, iiurlwidth=500, cllimit="max", uselang="en")
        raw = fetch(api("commons.wikimedia.org", **params))
        continuation, seen = (raw or {}).get("continue", {}), set()
        while continuation:
            token = encoded(continuation)
            if token in seen or len(seen) >= 50:
                raw = None
                break
            seen.add(token)
            following = fetch(api("commons.wikimedia.org", **params, **continuation))
            if not following:
                raw = None
                break
            previous = {page["pageid"]: page for page in raw["query"]["pages"]}
            for page in following.get("query", {}).get("pages", []):
                old = previous.get(page.get("pageid"))
                if old is None or old.get("revisions") != page.get("revisions") or old.get("imageinfo") != page.get("imageinfo"):
                    raw = None
                    break
                old.setdefault("categories", []).extend(page.get("categories", []))
            if raw is None:
                break
            continuation = following.get("continue", {})
        if not raw:
            for title in chunk:
                yield title, "commons-query-failed"
            continue
        for title, page, aliases in resolved_pages(raw, chunk):
            if page is None:
                yield title, "incomplete-commons-response"
            elif "missing" in page:
                yield title, dict(kind="commons", key=title, status="missing", proof=dict(page=page, aliases=aliases))
            elif page.get("imageinfo") and page.get("revisions"):
                info = page["imageinfo"][0]
                yield title, dict(kind="commons", key=title, status="present", identity=f"commons:{page['pageid']}", revision=page["revisions"][0]["revid"], filename=page["title"].removeprefix("File:"), pageid=page["pageid"], aliases=aliases, imageinfo=info, categories=sorted({category["title"] for category in page.get("categories", [])}), file_revision=dict(timestamp=info["timestamp"], sha1=info["sha1"]))
            else:
                yield title, "commons-file-data-missing"


def mediainfo(fetch, ids: list[str]):
    """Yield each `M<pageid>` with its `mediainfo` fact, or with the reason it has none."""
    for chunk in batches(ids):
        raw = fetch(api("commons.wikimedia.org", action="wbgetentities", ids="|".join(chunk), props="info|claims"))
        for key in chunk:
            value = (raw or {}).get("entities", {}).get(key)
            if value is None:
                yield key, "mediainfo-query-failed"
            elif "missing" in value:
                yield key, dict(kind="mediainfo", key=key, status="missing", proof=value)
            elif value.get("id") == key and value.get("lastrevid"):
                # Wikibase writes the statements of a file without statements as an empty list.
                depicts = (value.get("statements") or value.get("claims") or {}).get("P180", [])
                yield key, dict(kind="mediainfo", key=key, status="present", identity=key, revision=value["lastrevid"],
                                statements={"P180": [{field: statement[field] for field in ("mainsnak", "rank", "qualifiers") if field in statement} for statement in depicts]})
            else:
                yield key, "invalid-mediainfo-response"


def category_members(fetch, title: str) -> dict | str:
    """One nonrecursive response for a `Category:` title, bounded by photo policy, or the reason
    it has none."""
    limit = PHOTO_FALLBACK["files"]
    raw = fetch(api("commons.wikimedia.org", action="query", list="categorymembers", cmtitle=title, cmtype="file", cmlimit=limit, cmprop="ids|title|type"))
    members = (raw or {}).get("query", {}).get("categorymembers")
    if not isinstance(members, list) or len(members) > limit:
        return "invalid-category-response"
    return dict(kind="category", key=title, status="present", identity=title, members=members, bounded=True, limit=limit,
                limit_reached=len(members) == limit, truncated=bool(raw.get("continue")), continuation=raw.get("continue", {}))
