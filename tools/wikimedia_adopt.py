"""Validate retained capture facts without changing their source files."""
from html.parser import HTMLParser
import hashlib
import json
from pathlib import Path
import re
from urllib.parse import parse_qs, urlparse

try:
    from .landmark_capture import LANGUAGES, digest
    from .wikimedia_acquire import compact_entity, lead_html, original_notices, resolved_pages
except ImportError:
    from landmark_capture import LANGUAGES, digest
    from wikimedia_acquire import compact_entity, lead_html, original_notices, resolved_pages


class Licence(HTMLParser):
    def __init__(self):
        super().__init__()
        self.urls = set()
        self.stack = []
        self.footer_depth = None

    def handle_starttag(self, tag, attrs):
        href = dict(attrs).get("href", "")
        if dict(attrs).get("id") == "footer-info-copyright":
            self.footer_depth = len(self.stack)
        if self.footer_depth is not None and tag == "a" and "creativecommons.org/" in href:
            self.urls.add("https:" + href if href.startswith("//") else href)
        if self.footer_depth is not None and tag == "a" and ("Creative_Commons_Attribution-ShareAlike_4.0_International_License" in href or "Creative_Commons_Atribuci%C3%B3n-CompartirIgual_4.0_Internacional" in href):
            self.urls.add("https://creativecommons.org/licenses/by-sa/4.0/")
        if tag not in {"img", "meta", "link", "br", "hr", "input"}:
            self.stack.append(tag)

    def handle_endtag(self, tag):
        if tag in self.stack:
            depth = len(self.stack) - 1 - self.stack[::-1].index(tag)
            if self.footer_depth == depth:
                self.footer_depth = None
            del self.stack[depth:]


class RetainedCapture:
    def __init__(self, root: Path):
        self.root = root
        manifest = root / "manifest.json"
        if manifest.exists():
            self.sources = {source["path"]: source for source in json.loads(manifest.read_bytes())["sources"]}
        else:
            self.sources = {source["path"]: source for path in (root / "outcomes").glob("*.json")
                            if (source := json.loads(path.read_bytes())).get("status") == "ok"}

    def read(self, path: str) -> bytes:
        relative = Path(path)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("retained capture path leaves its root")
        source = self.sources[path]
        data = (self.root / path).read_bytes()
        if len(data) != source["bytes"] or digest(data) != source["sha256"]:
            raise ValueError(f"retained capture digest changed: {path}")
        return data

    def facts(self):
        """Only successful facts with an honest source revision are shared pins."""
        for path, source in sorted(self.sources.items()):
            if path.startswith(("entities/", "classes/", "locales/")) and path.endswith(".json"):
                raw = json.loads(self.read(path))
                for key, value in raw.get("entities", {}).items():
                    if not re.fullmatch(r"Q[1-9][0-9]*", key):
                        continue
                    base = dict(kind="entity", key=key, checked_at=source["retrieved_at"])
                    if "missing" in value:
                        yield dict(base, status="missing", proof=value)
                        continue
                    identity = value.get("id", key)
                    revision = value.get("lastrevid")
                    if not revision or not re.fullmatch(r"Q[1-9][0-9]*", identity):
                        continue
                    if identity != key and value.get("redirects") != {"from": key, "to": identity}:
                        continue
                    value = dict(value, id=identity)
                    yield dict(base, status="present", identity=identity, revision=revision, entity=compact_entity(value))
            elif path.startswith("articles/") and path.endswith(".json"):
                parsed = urlparse(source["url"])
                language = parsed.hostname.split(".")[0]
                title = parse_qs(parsed.query).get("titles", [None])[0]
                if language not in LANGUAGES or not title:
                    continue
                raw = json.loads(self.read(path))
                _, page, aliases = resolved_pages(raw, [title])[0]
                if not page or "missing" in page:
                    continue
                revisions = page.get("revisions", [])
                if not revisions:
                    continue
                html_path = str(Path(path).with_suffix(".html"))
                if html_path not in self.sources:
                    continue
                html = self.read(html_path).decode()
                revision = revisions[0]["revid"]
                stamp = re.search(r'"wgRevisionId"\s*:\s*(\d+)', html)
                if not stamp or int(stamp[1]) != revision:
                    raise ValueError(f"retained rendered revision mismatch: {html_path}")
                licenses = Licence()
                licenses.feed(html)
                usable = sorted(url for url in licenses.urls if re.fullmatch(r"https?://creativecommons.org/licenses/by-sa/(?:3\.0|4\.0)(?:/.*)?", url))
                if not usable:
                    continue
                yield dict(kind="article", key=f"{language}:{title}", status="present", checked_at=source["retrieved_at"], identity=f"{language}:{page['pageid']}",
                           revision=revision, language=language, title=page["title"], pageid=page["pageid"], qid=page.get("pageprops", {}).get("wikibase_item"),
                           aliases=aliases, timestamp=revisions[0]["timestamp"], url=self.sources[html_path]["url"], lead_html=lead_html(html),
                           license={"url": usable[0]}, original_notices=original_notices(html))
            elif path.startswith("links/") and path.endswith(".json"):
                parsed = urlparse(source["url"])
                params = parse_qs(parsed.query)
                raw = json.loads(self.read(path))
                if params.get("action") == ["wbgetentities"]:
                    for key, value in raw.get("entities", {}).items():
                        identity = value.get("id", "")
                        if "missing" in value or not value.get("lastrevid") or not re.fullmatch(r"Q[1-9][0-9]*", identity):
                            continue
                        if identity != key and value.get("redirects") != {"from": key, "to": identity}:
                            continue
                        yield dict(kind="link", key=f"wikidata:{key}", status="present", identity=identity, revision=value["lastrevid"],
                                   checked_at=source["retrieved_at"], proof=dict(redirects=value.get("redirects", {}), source_sha256=source["sha256"]))
                elif parsed.hostname.endswith(".wikipedia.org") and "continue" not in raw:
                    language = parsed.hostname.split(".")[0]
                    title = params.get("titles", [None])[0]
                    if not title:
                        continue
                    _, page, aliases = resolved_pages(raw, [title])[0]
                    identity = (page or {}).get("pageprops", {}).get("wikibase_item", "")
                    if re.fullmatch(r"Q[1-9][0-9]*", identity):
                        yield dict(kind="link", key=f"wikipedia:{language}:{title}", status="present", identity=identity, checked_at=source["retrieved_at"],
                                   proof=dict(language=language, pageid=page["pageid"], title=page["title"], aliases=aliases, langlinks=page.get("langlinks", []), source_sha256=source["sha256"]), sitelinks={})

    def files(self):
        """Verified prior conversion inputs retain file pins independently of metadata edits."""
        for path, source in sorted(self.sources.items()):
            if not path.startswith("images/") or Path(path).suffix not in {".jpg", ".png"}:
                continue
            metadata_path = str(Path(path).with_suffix(".json")).replace("-500.json", ".json")
            if metadata_path not in self.sources:
                continue
            raw = json.loads(self.read(metadata_path))
            pages = raw.get("query", {}).get("pages", {})
            pages = list(pages.values()) if isinstance(pages, dict) else pages
            if len(pages) != 1 or not pages[0].get("imageinfo"):
                continue
            page, info = pages[0], pages[0]["imageinfo"][0]
            data = self.read(path)
            if source["url"] == info["url"]:
                if hashlib.sha1(data).hexdigest() != info["sha1"]:
                    raise ValueError(f"retained file revision mismatch: {path}")
                input_kind = "original"
            elif source["url"] == info.get("thumburl") and info.get("thumbwidth") == 500:
                input_kind = "thumbnail500"
            else:
                continue
            yield dict(kind="file", key=page["title"], status="present", identity=f"commons:{page['pageid']}",
                       revision={key: info[key] for key in ("timestamp", "sha1")}, checked_at=source["retrieved_at"], filename=page["title"].removeprefix("File:"),
                       asset=dict(path=path, sha256=source["sha256"], bytes=source["bytes"], url=source["url"], input=input_kind)), self.root / path

    def unversioned_metadata(self) -> list[dict]:
        """A source digest proves old metadata bytes, but is not a page revision id."""
        result = []
        for path, source in sorted(self.sources.items()):
            if path.startswith("images/") and path.endswith(".json"):
                raw = json.loads(self.read(path))
                pages = raw.get("query", {}).get("pages", {})
                for page in pages.values() if isinstance(pages, dict) else pages:
                    if page.get("imageinfo") and not page.get("revisions"):
                        info = page["imageinfo"][0]
                        result.append(dict(filename=page["title"], path=path, sha256=source["sha256"],
                                           file_revision={key: info[key] for key in ("timestamp", "sha1")}, reason="description-page-revision-missing"))
        return result
