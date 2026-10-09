#!/usr/bin/env python3
"""Compact Wikimedia acquisition for the existing Store capture operation.

The caller resolves Store objects to verified inputs. This adapter writes only to the
operation's work and output directories; source snapshots own the admitted outputs.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
from html import escape
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import re
from urllib.parse import quote, urlencode

try:
    from .landmark_capture import Capture, LANGUAGES, api, batches, digest, write_json
except ImportError:
    from landmark_capture import Capture, LANGUAGES, api, batches, digest, write_json


KINDS = {"entity", "article", "commons", "link", "file", "category", "mediainfo"}
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


def verified(path: Path, sha256: str) -> dict:
    data = path.read_bytes()
    if digest(data) != sha256:
        raise ValueError(f"retained content digest changed: {path}")
    value = json.loads(data)
    if not isinstance(value, dict):
        raise ValueError(f"retained content is not an object: {path}")
    return value


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


class Acquisition:
    """A resumable operation over admitted pins, with no permanent cache of its own."""

    def __init__(self, work: Path, out: Path, check_id: str, inputs: list[dict], *, transport=None):
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,80}", check_id):
            raise ValueError("invalid acquisition check id")
        self.work, self.out, self.check_id = work, out, check_id
        work.mkdir(parents=True, exist_ok=True)
        out.mkdir(parents=True, exist_ok=True)
        (out / "manifest.json").unlink(missing_ok=True)
        self.transport = transport or Capture(work / "requests", interval=0.25)
        self.transport.retry_failed()
        self.records = {}
        self.failures = []
        self.inputs = {}
        self.used = set()
        self.assets = {}
        self.articles_by_revision = {}
        for record in inputs:
            value = verified(Path(record["path"]), record["sha256"])
            self.validate(record, value)
            self.inputs[(record["kind"], record["key"])] = (record, value)
            if record["kind"] == "article" and record["status"] == "present":
                self.articles_by_revision[(record["identity"], record["revision"])] = value
            if value.get("asset"):
                asset = value["asset"]
                asset_path = Path(record["asset_path"])
                data = asset_path.read_bytes()
                if len(data) != asset["bytes"] or digest(data) != asset["sha256"]:
                    raise ValueError("retained image digest changed")
                self.assets[asset["sha256"]] = asset_path
        for path in (work / "records" / check_id).glob("*.json"):
            record = json.loads(path.read_bytes())
            value = verified(work / record["path"], record["sha256"])
            self.validate(record, value)
            pair = (record["kind"], record["key"])
            incoming = self.inputs.get(pair)
            if incoming and datetime.fromisoformat(incoming[0]["checked_at"].replace("Z", "+00:00")) >= datetime.fromisoformat(record["checked_at"].replace("Z", "+00:00")):
                continue
            self.records[pair] = record
            if record["kind"] == "article" and record["status"] == "present":
                self.articles_by_revision[(record["identity"], record["revision"])] = value

    def adopt(self, root: Path):
        if __package__:
            from .wikimedia_adopt import RetainedCapture
        else:
            from wikimedia_adopt import RetainedCapture
        capture = RetainedCapture(root)
        for value in capture.facts():
            pair = (value["kind"], value["key"])
            if pair not in self.records and pair not in self.inputs:
                self.keep(value, input_only=True)
        for value, path in capture.files():
            asset = value["asset"]
            relative = f"assets/{asset['sha256']}{path.suffix}"
            target = self.work / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            if not target.exists():
                os.link(path, target)
            self.assets[asset["sha256"]] = target
            pair = (value["kind"], value["key"])
            if pair not in self.records and pair not in self.inputs:
                self.keep(dict(value, asset=dict(asset, path=relative)), input_only=True)
        self.used.clear()

    @staticmethod
    def validate(record: dict, value: dict):
        if record["kind"] not in KINDS or value.get("kind") != record["kind"] or value.get("key") != record["key"]:
            raise ValueError("retained content identity changed")
        for field in ("identity", "revision", "status"):
            if value.get(field) != record.get(field):
                raise ValueError(f"retained content {field} changed")
        if record["status"] not in {"present", "missing"}:
            raise ValueError("unresolved content is not an admitted pin")
        if record["status"] == "present" and record["kind"] in {"entity", "article", "commons", "mediainfo"}:
            revision = value.get("revision")
            if type(revision) is not int or revision <= 0 or not value.get("identity"):
                raise ValueError("retained content has no valid revision pin")
        if value.get("asset"):
            path = Path(value["asset"]["path"])
            if path.is_absolute() or ".." in path.parts:
                raise ValueError("retained asset path leaves output directory")

    def reuse(self, kind: str, key: str, refresh: bool) -> bool:
        pair = (kind, key)
        self.used.add(pair)
        if pair in self.records:
            return True
        if not refresh and pair in self.inputs:
            record, value = self.inputs[pair]
            self.keep(dict(value, checked_at=record["checked_at"]))
            return True
        return False

    def keep(self, value: dict, *, input_only=False):
        self.validate(value, value)
        data = encoded({key: item for key, item in value.items() if key != "checked_at"})
        if len(data) > 16 * 1024 * 1024:
            self.fail(value["kind"], value["key"], "compact-content-exceeds-bound")
            return
        sha256 = digest(data)
        relative = f"content/{value['kind']}/{sha256}.json"
        target = self.work / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        temporary = target.with_suffix(".tmp")
        temporary.write_bytes(data)
        temporary.replace(target)
        record = {field: value[field] for field in ("kind", "key", "status", "checked_at")}
        record.update({field: value[field] for field in ("identity", "revision") if field in value})
        record.update(path=relative, sha256=sha256)
        if value["kind"] == "article" and value["status"] == "present":
            self.articles_by_revision[(value["identity"], value["revision"])] = value
        if input_only:
            self.inputs[(value["kind"], value["key"])] = (dict(record, path=str(target)), value)
            return
        write_json(self.work / "records" / self.check_id / (digest(f"{value['kind']}:{value['key']}".encode()) + ".json"), record)
        self.records[(value["kind"], value["key"])] = record
        self.used.add((value["kind"], value["key"]))

    def value(self, kind: str, key: str) -> dict | None:
        record = self.records.get((kind, key))
        return verified(self.work / record["path"], record["sha256"]) if record else None

    def json(self, url: str) -> dict | None:
        return self.transport.json(f"{self.check_id}/{digest(url.encode())}.json", url)

    def entity_batches(self, ids: list[str], props: str):
        for chunk in batches(ids):
            url = api("www.wikidata.org", action="wbgetentities", ids="|".join(chunk), redirects="yes", props=props,
                      languages="|".join(LANGUAGES), sitefilter="|".join(language + "wiki" for language in LANGUAGES))
            raw = self.json(url)
            oversized = False
            if isinstance(self.transport, Capture):
                path = f"{self.check_id}/{digest(url.encode())}.json"
                outcome = json.loads((self.transport.root / "outcomes" / (digest(path.encode()) + ".json")).read_bytes())
                oversized = outcome["status"] == "oversized" or outcome.get("bytes", 0) > 16 * 1024 * 1024
            if oversized and len(chunk) > 1:
                half = len(chunk) // 2
                yield from self.entity_batches(chunk[:half], props)
                yield from self.entity_batches(chunk[half:], props)
            else:
                yield chunk, raw

    def fail(self, kind: str, key: str, reason: str):
        self.failures.append(dict(kind=kind, key=key, reason=reason))

    def base(self, kind: str, key: str, status: str, **fields) -> dict:
        return dict(kind=kind, key=key, status=status, checked_at=datetime.now(timezone.utc).isoformat(), **fields)

    def entities(self, ids: list[str], refresh=False):
        missing = [qid for qid in sorted(set(ids)) if not self.reuse("entity", qid, refresh)]
        if any(not re.fullmatch(r"Q[1-9][0-9]*", qid) for qid in missing):
            raise ValueError("invalid Wikidata id")
        fresh, check = [], []
        for qid in missing:
            (check if refresh and ("entity", qid) in self.inputs else fresh).append(qid)
        for chunk, raw in self.entity_batches(check, "info"):
            for qid in chunk:
                value = (raw or {}).get("entities", {}).get(qid)
                if not value:
                    self.fail("entity", qid, "entity-freshness-check-failed")
                    continue
                previous, compact = self.inputs[("entity", qid)]
                if "missing" in value:
                    self.keep(self.base("entity", qid, "missing", proof=value))
                elif previous.get("identity") == value.get("id") and previous.get("revision") == value.get("lastrevid"):
                    self.keep(dict(compact, checked_at=datetime.now(timezone.utc).isoformat()))
                else:
                    fresh.append(qid)
        for chunk, raw in self.entity_batches(fresh, "info|labels|claims|sitelinks"):
            values = raw.get("entities", {}) if raw else {}
            for qid in chunk:
                value = values.get(qid)
                if value is None:
                    self.fail("entity", qid, "incomplete-entity-response")
                elif "missing" in value:
                    self.keep(self.base("entity", qid, "missing", proof=value))
                elif value.get("lastrevid") and re.fullmatch(r"Q[1-9][0-9]*", value.get("id", "")):
                    if value["id"] != qid and value.get("redirects") != {"from": qid, "to": value["id"]}:
                        self.fail("entity", qid, "entity-redirect-proof-missing")
                        continue
                    self.keep(self.base("entity", qid, "present", identity=value["id"], revision=value["lastrevid"], entity=compact_entity(value)))
                else:
                    self.fail("entity", qid, "invalid-entity-response")

    def dependencies(self, subjects: list[str], refresh=False):
        """Shared class closure and the compiler's bounded administrative/country inputs."""
        self.entities(subjects, refresh)

        def ids(qid, prop):
            record = self.value("entity", qid)
            claims = (record or {}).get("entity", {}).get("claims", {}).get(prop, [])
            return {claim.get("mainsnak", {}).get("datavalue", {}).get("value", {}).get("id")
                    for claim in claims if isinstance(claim.get("mainsnak", {}).get("datavalue", {}).get("value"), dict)} - {None}

        pending = set().union(*(ids(qid, "P31") for qid in subjects))
        visited = set()
        while pending:
            if len(visited | pending) > 65536:
                raise ValueError("class closure exceeds acquisition bound")
            self.entities(sorted(pending), refresh)
            visited |= pending
            pending = set().union(*(ids(qid, "P279") for qid in pending)) - visited
        countries = set().union(*(ids(qid, "P17") for qid in subjects))
        self.entities(sorted(countries), refresh)
        frontiers = {qid: ids(qid, "P131") for qid in subjects}
        visited = {qid: set() for qid in subjects}
        for _ in range(8):
            for qid in frontiers:
                frontiers[qid] = set(sorted(frontiers[qid] - visited[qid])[:64 - len(visited[qid])])
            pending = set().union(*frontiers.values())
            if not pending:
                break
            self.entities(sorted(pending), refresh)
            for qid, frontier in frontiers.items():
                visited[qid] |= frontier
                frontiers[qid] = set().union(*(ids(locale, "P131") for locale in frontier)) - visited[qid]

    def pages(self, language: str, titles: list[str]) -> dict:
        """Resolve exact links and collect every required language-link page."""
        result = {}
        for chunk in batches(sorted(set(titles))):
            params = dict(action="query", formatversion=2, titles="|".join(chunk), redirects=1,
                          prop="pageprops|langlinks|revisions", rvprop="ids|timestamp", lllimit="max")
            combined, continuation, seen = None, {}, set()
            for _ in range(50):
                raw = self.json(api(f"{language}.wikipedia.org", **params, **continuation))
                if not raw:
                    combined = None
                    break
                if combined is None:
                    combined = raw
                else:
                    previous = {page["pageid"]: page for page in combined["query"]["pages"]}
                    for page in raw.get("query", {}).get("pages", []):
                        old = previous.get(page.get("pageid"))
                        if old is None or old.get("revisions") != page.get("revisions"):
                            combined = None
                            break
                        old.setdefault("langlinks", []).extend(page.get("langlinks", []))
                    if combined is None:
                        break
                continuation = raw.get("continue", {})
                if not continuation:
                    break
                token = encoded(continuation)
                if token in seen:
                    combined = None
                    break
                seen.add(token)
            else:
                combined = None
            if combined is not None:
                result.update({title: (page, aliases) for title, page, aliases in resolved_pages(combined, chunk)})
        return result

    def links(self, links: list[str], refresh=False) -> list[str]:
        """Resolve explicit OSM identities without retaining their regional associations."""
        pending = [link for link in sorted(set(links)) if not self.reuse("link", link, refresh)]
        qids, groups = [], {}
        for link in pending:
            kind, _, value = link.partition(":")
            if kind == "wikidata" and re.fullmatch(r"Q[1-9][0-9]*", value):
                qids.append(value)
            elif kind == "wikipedia":
                language, _, title = value.partition(":")
                if not re.fullmatch(r"[a-z][a-z-]{1,11}", language) or not title or "|" in title:
                    raise ValueError("invalid exact Wikipedia link")
                groups.setdefault(language, []).append(title)
            else:
                raise ValueError("invalid explicit content link")
        self.entities(qids, refresh)
        for qid in qids:
            value = self.value("entity", qid)
            key = f"wikidata:{qid}"
            if not value:
                self.fail("link", key, "link-entity-failed")
            elif value["status"] == "missing":
                self.keep(self.base("link", key, "missing", proof=value))
            else:
                self.keep(self.base("link", key, "present", identity=value["identity"], revision=value["revision"], proof=value["entity"].get("redirects", {})))
        pages = {(language, title): value for language, titles in groups.items() for title, value in self.pages(language, titles).items()}
        canonical = {}
        for pair, (page, aliases) in pages.items():
            if not page or "missing" in page or page.get("pageprops", {}).get("wikibase_item"):
                continue
            language, title = pair
            sitelinks = {item["lang"]: item.get("title", item.get("*")) for item in page.get("langlinks", [])}
            sitelinks[language] = page["title"]
            selected = next((code for code in LANGUAGES if code in sitelinks), language)
            target = (selected, sitelinks[selected])
            canonical[pair] = (target, sitelinks)
        needed = {}
        for target, _ in canonical.values():
            if target not in pages:
                needed.setdefault(target[0], []).append(target[1])
        pages.update({(language, title): value for language, titles in needed.items() for title, value in self.pages(language, titles).items()})
        for language, titles in groups.items():
            for title in titles:
                key, pair = f"wikipedia:{language}:{title}", (language, title)
                page, aliases = pages.get(pair, (None, []))
                if page is None:
                    self.fail("link", key, "incomplete-language-link-response")
                    continue
                if "missing" in page:
                    self.keep(self.base("link", key, "missing", proof=dict(page=page, aliases=aliases)))
                    continue
                if type(page.get("pageid")) is not int or page["pageid"] <= 0:
                    self.fail("link", key, "invalid-page-identity")
                    continue
                qid = page.get("pageprops", {}).get("wikibase_item")
                if qid and not re.fullmatch(r"Q[1-9][0-9]*", qid):
                    self.fail("link", key, "invalid-page-item")
                    continue
                proof = dict(language=language, pageid=page["pageid"], title=page["title"], aliases=aliases, langlinks=page.get("langlinks", []))
                selected_page = page
                selected = language
                if not qid:
                    target, sitelinks = canonical[pair]
                    selected = target[0]
                    selected_page, selected_aliases = pages.get(target, (None, []))
                    if not selected_page or "missing" in selected_page:
                        self.fail("link", key, "canonical-language-link-failed")
                        continue
                    if type(selected_page.get("pageid")) is not int or selected_page["pageid"] <= 0:
                        self.fail("link", key, "invalid-canonical-page-identity")
                        continue
                    qid = selected_page.get("pageprops", {}).get("wikibase_item")
                    if qid and not re.fullmatch(r"Q[1-9][0-9]*", qid):
                        self.fail("link", key, "invalid-canonical-page-item")
                        continue
                    proof["canonical"] = dict(language=selected, pageid=selected_page["pageid"], title=selected_page["title"], aliases=selected_aliases, langlinks=selected_page.get("langlinks", []))
                identity = qid or f"wiki-{selected}-{selected_page['pageid']}"
                self.keep(self.base("link", key, "present", identity=identity, revision=selected_page.get("revisions", [{}])[0].get("revid"), proof=proof,
                                    sitelinks={code: {"title": linked_title} for code, linked_title in (canonical.get(pair, (None, {language: page["title"]}))[1]).items() if code in LANGUAGES}))
        resolved = {record["identity"] for pair, record in self.records.items() if pair in self.used and pair[0] == "link" and record["status"] == "present" and record["identity"].startswith("Q")}
        self.entities(sorted(resolved), refresh)
        for pair, record in list(self.records.items()):
            if pair not in self.used or pair[0] != "link" or record["status"] != "present" or not record["identity"].startswith("Q"):
                continue
            subject = self.value("entity", record["identity"])
            if not subject or subject["status"] != "present":
                self.fail("link", pair[1], "canonical-subject-unresolved")
                del self.records[pair]
            elif subject["identity"] != record["identity"]:
                value = self.value("link", pair[1])
                value["proof"]["entity_redirect"] = subject["entity"]["redirects"]
                self.keep(dict(value, identity=subject["identity"], checked_at=record["checked_at"]))
        return sorted({record["identity"] for pair, record in self.records.items() if pair in self.used and pair[0] == "link" and record["status"] == "present"})

    def articles(self, requests: list[dict], refresh=False):
        groups = {}
        for request in requests:
            language, title = request["language"], request["title"]
            if language not in LANGUAGES or not title or "|" in title:
                raise ValueError("invalid article identity")
            key = f"{language}:{title}"
            if not self.reuse("article", key, refresh):
                groups.setdefault(language, set()).add(title)
        for language, titles in groups.items():
            for chunk in batches(sorted(titles)):
                raw = self.json(api(f"{language}.wikipedia.org", action="query", formatversion=2, titles="|".join(chunk), redirects=1, prop="pageprops|revisions", rvprop="ids|timestamp", rvslots="main"))
                if not raw:
                    for title in chunk:
                        self.fail("article", f"{language}:{title}", "article-query-failed")
                    continue
                for title, page, aliases in resolved_pages(raw, chunk):
                    key = f"{language}:{title}"
                    if page is None:
                        self.fail("article", key, "incomplete-page-response")
                        continue
                    if "missing" in page:
                        self.keep(self.base("article", key, "missing", proof=dict(page=page, aliases=aliases)))
                        continue
                    revisions = page.get("revisions", [])
                    if not revisions:
                        self.fail("article", key, "article-revision-missing")
                        continue
                    qid = page.get("pageprops", {}).get("wikibase_item")
                    if qid is not None and (not isinstance(qid, str) or not re.fullmatch(r"Q[1-9][0-9]*", qid)):
                        self.fail("article", key, "invalid-page-item")
                        continue
                    revision = revisions[0]["revid"]
                    identity = f"{language}:{page['pageid']}"
                    previous = self.articles_by_revision.get((identity, revision))
                    if previous:
                        value = dict(previous, key=key, checked_at=datetime.now(timezone.utc).isoformat(), aliases=aliases,
                                     qid=page.get("pageprops", {}).get("wikibase_item"), title=page["title"],
                                     url=f"https://{language}.wikipedia.org/w/index.php?" + urlencode(dict(title=page["title"], oldid=revision)))
                        self.keep(value)
                        continue
                    value = self.json(f"https://{language}.wikipedia.org/w/rest.php/v1/page/{quote(page['title'], safe='')}/with_html")
                    if not value:
                        self.fail("article", key, "article-html-failed")
                        continue
                    if value.get("id") != page["pageid"] or value.get("latest", {}).get("id") != revision:
                        self.fail("article", key, "article-changed-during-acquisition")
                        continue
                    try:
                        html = lead_html(value["html"])
                        license = value["license"]
                    except (KeyError, TypeError, ValueError) as error:
                        self.fail("article", key, f"invalid-article-lead: {error}")
                        continue
                    self.keep(self.base("article", key, "present", identity=identity, revision=revision, language=language, title=value["title"], pageid=value["id"], qid=page.get("pageprops", {}).get("wikibase_item"), aliases=aliases, timestamp=value["latest"]["timestamp"], url=f"https://{language}.wikipedia.org/w/index.php?" + urlencode(dict(title=value["title"], oldid=revision)), lead_html=html, license=license, original_notices=original_notices(value["html"])))

    def commons(self, filenames: list[str], refresh=False):
        titles = sorted({"File:" + filename.removeprefix("File:").replace("_", " ") for filename in filenames})
        if any("|" in title or title == "File:" for title in titles):
            raise ValueError("invalid Commons filename")
        missing = [title for title in titles if not self.reuse("commons", title, refresh)]
        for chunk in batches(missing):
            params = dict(action="query", formatversion=2, titles="|".join(chunk), redirects=1, prop="imageinfo|revisions|categories", rvprop="ids|timestamp", iiprop="url|timestamp|sha1|extmetadata|mime|size", iilimit=1, iiurlwidth=500, cllimit="max", uselang="en")
            raw = self.json(api("commons.wikimedia.org", **params))
            continuation, seen = (raw or {}).get("continue", {}), set()
            while continuation:
                token = encoded(continuation)
                if token in seen or len(seen) >= 50:
                    raw = None
                    break
                seen.add(token)
                following = self.json(api("commons.wikimedia.org", **params, **continuation))
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
                    self.fail("commons", title, "commons-query-failed")
                continue
            for title, page, aliases in resolved_pages(raw, chunk):
                if page is None:
                    self.fail("commons", title, "incomplete-commons-response")
                elif "missing" in page:
                    self.keep(self.base("commons", title, "missing", proof=dict(page=page, aliases=aliases)))
                elif page.get("imageinfo") and page.get("revisions"):
                    info = page["imageinfo"][0]
                    self.keep(self.base("commons", title, "present", identity=f"commons:{page['pageid']}", revision=page["revisions"][0]["revid"], filename=page["title"].removeprefix("File:"), pageid=page["pageid"], aliases=aliases, imageinfo=info, categories=sorted({category["title"] for category in page.get("categories", [])}), file_revision=dict(timestamp=info["timestamp"], sha1=info["sha1"])))
                else:
                    self.fail("commons", title, "commons-file-data-missing")
        media = sorted({f"M{value['pageid']}" for title in titles if (value := self.value("commons", title)) and value["status"] == "present"
                        and not self.reuse("mediainfo", f"M{value['pageid']}", refresh)})
        for chunk in batches(media):
            raw = self.json(api("commons.wikimedia.org", action="wbgetentities", ids="|".join(chunk), props="info|claims"))
            for key in chunk:
                value = (raw or {}).get("entities", {}).get(key)
                if value is None:
                    self.fail("mediainfo", key, "mediainfo-query-failed")
                elif "missing" in value:
                    self.keep(self.base("mediainfo", key, "missing", proof=value))
                elif value.get("id") == key and value.get("lastrevid"):
                    depicts = value.get("statements", value.get("claims", {})).get("P180", [])
                    self.keep(self.base("mediainfo", key, "present", identity=key, revision=value["lastrevid"],
                                        statements={"P180": [{field: statement[field] for field in ("mainsnak", "rank", "qualifiers") if field in statement} for statement in depicts]}))
                else:
                    self.fail("mediainfo", key, "invalid-mediainfo-response")

    def categories(self, categories: list[str], refresh=False):
        """One nonrecursive response per directly claimed category, bounded by photo policy."""
        if not categories:
            return
        if __package__:
            from .landmark_capture import PHOTO_FALLBACK
        else:
            from landmark_capture import PHOTO_FALLBACK
        limit = PHOTO_FALLBACK["files"]
        for category in sorted(set(categories)):
            title = "Category:" + category.removeprefix("Category:").replace("_", " ")
            if not title.removeprefix("Category:") or "|" in title:
                raise ValueError("invalid Commons category")
            if self.reuse("category", title, refresh):
                continue
            raw = self.json(api("commons.wikimedia.org", action="query", list="categorymembers", cmtitle=title, cmtype="file", cmlimit=limit, cmprop="ids|title|type"))
            members = (raw or {}).get("query", {}).get("categorymembers")
            if not isinstance(members, list) or len(members) > limit:
                self.fail("category", title, "invalid-category-response")
                continue
            self.keep(self.base("category", title, "present", identity=title, members=members, bounded=True, limit=limit,
                                limit_reached=len(members) == limit, truncated=bool(raw.get("continue")), continuation=raw.get("continue", {})))

    def finish(self) -> dict:
        records = []
        assets = []
        for pair, record in sorted(self.records.items()):
            if pair not in self.used:
                continue
            source = self.work / record["path"]
            verified(source, record["sha256"])
            target = self.out / record["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            if not target.exists():
                os.link(source, target)
            verified(target, record["sha256"])
            records.append(record)
            value = verified(source, record["sha256"])
            if value.get("asset"):
                asset = value["asset"]
                asset_source = self.assets.get(asset["sha256"], self.work / asset["path"])
                data = asset_source.read_bytes()
                if len(data) != asset["bytes"] or digest(data) != asset["sha256"]:
                    raise ValueError("retained image digest changed")
                asset_target = self.out / asset["path"]
                asset_target.parent.mkdir(parents=True, exist_ok=True)
                if not asset_target.exists():
                    os.link(asset_source, asset_target)
                assets.append(asset)
        manifest = dict(schema=1, complete=not self.failures, records=records, assets=assets, failures=self.failures)
        if isinstance(getattr(self.transport, "metrics", None), dict):
            manifest["acquisition"] = dict(self.transport.metrics)
        write_json(self.out / "manifest.json", manifest)
        return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--requests", type=Path, required=True)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--adopt", type=Path, action="append", default=[])
    parser.add_argument("--catalog", type=Path)
    parser.add_argument("--allow-media", action="store_true")
    args = parser.parse_args()
    requests = json.loads(args.requests.read_bytes())
    if args.catalog:
        if __package__:
            from .wikimedia_snapshot import SnapshotAcquisition
        else:
            from wikimedia_snapshot import SnapshotAcquisition
        operation = SnapshotAcquisition(args.work, args.out, requests["check_id"], json.loads(args.inputs.read_bytes()),
                                        catalog=args.catalog, allow_media=args.allow_media)
    else:
        operation = Acquisition(args.work, args.out, requests["check_id"], json.loads(args.inputs.read_bytes()))
    try:
        for capture_root in args.adopt:
            operation.adopt(capture_root)
        refresh = requests.get("refresh", False)
        identities = operation.links(requests.get("links", []), refresh)
        operation.dependencies(sorted(set(requests.get("entities", [])) | {identity for identity in identities if identity.startswith("Q")}), refresh)
        operation.articles(requests.get("articles", []), requests.get("refresh", False))
        operation.commons(requests.get("commons", []), requests.get("refresh", False))
        operation.categories(requests.get("categories", []), requests.get("refresh", False))
        for filename in requests.get("files", []):
            key = "File:" + filename.removeprefix("File:").replace("_", " ")
            if not operation.reuse("file", key, False):
                if args.catalog and args.allow_media:
                    if operation.db.execute("SELECT 1 FROM facts WHERE kind='file' AND key=?", (key,)).fetchone():
                        operation.load("file", [key])
                elif args.catalog:
                    operation.load("file", [key])
                else:
                    operation.fail("file", key, "image-input-not-retained")
    except (RuntimeError, ValueError, KeyError, TypeError) as error:
        operation.fail("operation", requests["check_id"], str(error))
    return 0 if operation.finish()["complete"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
