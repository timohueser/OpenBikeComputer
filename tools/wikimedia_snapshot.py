#!/usr/bin/env python3
"""Stream Wikimedia snapshots into a compact, indexed content source."""
from __future__ import annotations

import argparse
import bz2
from contextlib import contextmanager
from datetime import datetime, timezone
import gzip
import hashlib
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import tarfile
from urllib.parse import urlencode, urlparse
from urllib.request import Request, HTTPRedirectHandler, build_opener

try:
    from .wikimedia_acquire import Acquisition, compact_entity, encoded, lead_html, original_notices
    from .landmark_capture import LANGUAGES, USER_AGENT, write_json
    from . import wikimedia_snapshot_sql as bulk_sql
except ImportError:
    from wikimedia_acquire import Acquisition, compact_entity, encoded, lead_html, original_notices
    from landmark_capture import LANGUAGES, USER_AGENT, write_json
    import wikimedia_snapshot_sql as bulk_sql

BOUND = 16 * 1024 * 1024
RESERVE = 4 * 1024**3


def checkpoint():
    control = os.environ.get("OBC_CONTENT_CONTROL")
    if control:
        state = json.loads(Path(control).read_bytes())["state"]
        if state.get("state") in {"stopping", "stopped"}:
            raise RuntimeError("content preparation stopped; successful work is retained")


class Redirects(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, url):
        if not url.startswith("https://"):
            raise ValueError("snapshot redirect must use HTTPS")
        redirected = super().redirect_request(request, response, code, message, headers, url)
        if redirected and urlparse(request.full_url).hostname != urlparse(url).hostname:
            redirected.remove_header("Authorization")
        return redirected


def open_url(request):
    return build_opener(Redirects()).open(request, timeout=60)


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while data := stream.read(1024 * 1024):
            checkpoint()
            digest.update(data)
    return digest.hexdigest()


def connect(path: Path):
    db = sqlite3.connect(path)
    db.execute("PRAGMA cache_size=-16384")
    db.execute("PRAGMA temp_store=FILE")
    return db


def schema(db):
    bulk_sql.schema(db)
    db.executescript("""
        CREATE TABLE IF NOT EXISTS facts(kind TEXT, key TEXT, revision INTEGER, data BLOB,
            PRIMARY KEY(kind,key)) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS imports(id TEXT PRIMARY KEY, sha256 TEXT, complete INTEGER, records INTEGER);
        CREATE TABLE IF NOT EXISTS kept(kind TEXT, key TEXT, PRIMARY KEY(kind,key)) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS assets(sha256 TEXT PRIMARY KEY, path TEXT, bytes INTEGER);
        CREATE TABLE IF NOT EXISTS rejected(kind TEXT,key TEXT,revision INTEGER,reason TEXT,
            PRIMARY KEY(kind,key)) WITHOUT ROWID;
    """)


def put(db, value):
    Acquisition.validate(value, value)
    data = encoded(value)
    if len(data) > BOUND:
        raise ValueError(f"compact fact exceeds limit: {value['kind']}:{value['key']}")
    revision = value.get("revision", 0)
    if not isinstance(revision, int):
        revision = 0
    db.execute("""INSERT INTO facts VALUES(?,?,?,?) ON CONFLICT(kind,key) DO UPDATE SET
        revision=excluded.revision,data=excluded.data WHERE excluded.revision >= facts.revision""",
        (value["kind"], value["key"], revision, data))
    if value["kind"] == "entity" and value["status"] == "present":
        for wiki, link in value["entity"]["sitelinks"].items():
            db.execute("INSERT OR REPLACE INTO sitelinks VALUES(?,?,?)", (wiki.removesuffix("wiki"), link["title"], value["identity"]))


def fact(db, kind, key):
    row = db.execute("SELECT data FROM facts WHERE kind=? AND key=?", (kind, key)).fetchone()
    return json.loads(row[0]) if row else None


@contextmanager
def records(path: Path):
    """Decompress one record at a time. Never extract an archive member to disk."""
    if path.name.endswith((".tar.gz", ".tgz")):
        with tarfile.open(path, "r|gz") as archive:
            found = False
            for member in archive:
                if not member.isfile() or not member.name.endswith((".ndjson", ".json")):
                    raise ValueError("snapshot must contain only its NDJSON data member")
                if found:
                    raise ValueError("snapshot contains multiple data members")
                found = True
                with archive.extractfile(member) as stream:
                    yield stream
            if not found:
                raise ValueError("snapshot contains no data member")
    else:
        opener = bz2.open if path.suffix == ".bz2" else gzip.open if path.suffix == ".gz" else open
        with opener(path, "rb") as stream:
            yield stream


def rows(stream):
    while line := stream.readline(128 * 1024 * 1024 + 1):
        if len(line) > 128 * 1024 * 1024:
            raise ValueError("snapshot record exceeds input limit")
        line = line.strip()
        if line in (b"[", b"]", b""):
            continue
        value = json.loads(line.removesuffix(b","))
        if not isinstance(value, dict):
            raise ValueError("snapshot line is not an object")
        yield value


def download(source, work):
    if "path" in source:
        path = Path(source["path"]).resolve()
    else:
        url = source["url"]
        if not url.startswith("https://"):
            raise ValueError("snapshot downloads require HTTPS")
        token = os.environ.get("OBC_WIKIMEDIA_ENTERPRISE_TOKEN")
        headers = {"User-Agent": USER_AGENT, "Accept-Encoding": "identity"}
        if url.startswith("https://api.enterprise.wikimedia.com/"):
            if not token:
                raise ValueError("set OBC_WIKIMEDIA_ENTERPRISE_TOKEN for snapshot downloads")
            headers["Authorization"] = f"Bearer {token}"
        # Resolve the signed storage URL before downloading; never forward its API token.
        with open_url(Request(url, headers=headers, method="HEAD")) as response:
            target = response.url
            length = int(response.headers["Content-Length"])
            etag = response.headers.get("ETag")
        identity = hashlib.sha256(url.encode()).hexdigest()
        path = work / (identity + source.get("suffix", ".tar.gz"))
        receipt = work / (identity + ".json")
        previous = json.loads(receipt.read_bytes()) if receipt.exists() else None
        current = dict(url=url, bytes=length, etag=etag)
        if previous != current:
            path.unlink(missing_ok=True)
            write_json(receipt, current)
        offset = path.stat().st_size if path.exists() else 0
        if offset > length:
            raise ValueError("partial snapshot exceeds declared length")
        if offset < length:
            if shutil.disk_usage(work).free < length - offset + RESERVE:
                raise ValueError("snapshot download would consume the disk reserve")
            transfer = {"User-Agent": USER_AGENT, "Accept-Encoding": "identity"}
            if target == url and "Authorization" in headers:
                transfer["Authorization"] = headers["Authorization"]
            if offset:
                transfer["Range"] = f"bytes={offset}-"
            with open_url(Request(target, headers=transfer)) as response:
                if etag and response.headers.get("ETag", etag) != etag:
                    raise ValueError("upstream changed the pinned snapshot during download")
                if offset and (response.status != 206 or response.headers.get("Content-Range") != f"bytes {offset}-{length-1}/{length}"):
                    raise ValueError("upstream did not resume the pinned snapshot")
                with path.open("ab" if offset else "wb") as output:
                    while data := response.read(1024 * 1024):
                        checkpoint()
                        output.write(data)
                    output.flush()
                    os.fsync(output.fileno())
            if path.stat().st_size != length:
                raise ValueError("incomplete snapshot download; run preparation again to resume")
    if not path.is_file():
        raise ValueError(f"snapshot file missing: {path}")
    digest = sha(path)
    if source.get("sha256") and source["sha256"] != digest:
        raise ValueError("snapshot checksum changed")
    if source.get("bytes") is not None and source["bytes"] != path.stat().st_size:
        raise ValueError("snapshot length changed")
    return path, digest


def entity(raw, checked_at):
    if raw.get("type") not in (None, "item") or not re.fullmatch(r"Q[1-9][0-9]*", raw.get("id", "")):
        return None
    return dict(kind="entity", key=raw["id"], status="present", identity=raw["id"],
                revision=raw["lastrevid"], checked_at=checked_at, entity=compact_entity(raw))


def article(raw, language, checked_at):
    if (raw.get("namespace") or {}).get("identifier", 0) != 0:
        return []
    title, pageid = raw.get("name"), raw.get("identifier")
    declared_revision = (raw.get("version") or {}).get("identifier")
    if not isinstance(title, str) or type(pageid) is not int or pageid <= 0:
        raise ValueError("invalid snapshot page identity")
    html = (raw.get("article_body") or {}).get("html")
    licenses = raw.get("license") or []
    if isinstance(licenses, dict):
        licenses = [licenses]
    license = next((item for item in licenses if isinstance(item, dict) and "creativecommons.org/licenses/by-sa/" in (item.get("url") or "")), None)
    if not isinstance(html, str) or not license:
        raise ValueError(f"snapshot article lacks revision-bound HTML or licence: {language}:{title}")
    class Header(HTMLParser):
        about = None
        pageid = None
        def handle_starttag(self, tag, attrs):
            attrs = dict(attrs)
            if tag == "html":
                self.about = attrs.get("about")
            if tag == "meta" and attrs.get("property") == "mw:pageId":
                self.pageid = attrs.get("content")
    header = Header()
    header.feed(html)
    match = re.fullmatch(rf"https://{re.escape(language)}\.wikipedia\.org/wiki/Special:Redirect/revision/([1-9][0-9]*)", header.about or "")
    if not match or header.pageid != str(pageid):
        raise ValueError("rendered HTML has no matching page and revision proof")
    revision = int(match[1])
    if (raw.get("in_language") or {}).get("identifier") != language:
        raise ValueError("snapshot article language differs from its source")
    qid = (raw.get("main_entity") or {}).get("identifier")
    if qid is not None and not re.fullmatch(r"Q[1-9][0-9]*", str(qid)):
        raise ValueError("invalid article Wikidata identity")
    aliases = [item["name"] for item in raw.get("redirects") or [] if item.get("name")]
    result = []
    for requested in dict.fromkeys([title, *aliases]):
        key = f"{language}:{requested}"
        proof = [] if requested == title else [dict(from_=requested, to=title)]
        proof = [{"from": item.pop("from_"), **item} for item in proof]
        value = dict(kind="article", key=key, status="present", identity=f"{language}:{pageid}", revision=revision,
            checked_at=checked_at, language=language, title=title, pageid=pageid, qid=qid, aliases=proof,
            timestamp=raw.get("date_modified") if revision == declared_revision else None, url=f"https://{language}.wikipedia.org/w/index.php?" + urlencode(dict(title=title, oldid=revision)),
            lead_html=lead_html(html), license=license, original_notices=original_notices(html))
        value["representation"] = dict(about=header.about, pageid=pageid, revision=revision, declared_revision=declared_revision,
                                       html_sha256=hashlib.sha256(html.encode()).hexdigest())
        result.append(value)
        result.append(dict(kind="link", key=f"wikipedia:{language}:{requested}", status="present",
            identity=qid or f"wiki-{language}-{pageid}", revision=revision, checked_at=checked_at,
            proof=dict(language=language, title=title, pageid=pageid, aliases=proof, snapshot=True),
            sitelinks={language + "wiki": {"title": title}}))
    return result


def import_sources(config, work):
    work.mkdir(parents=True, exist_ok=True)
    db = connect(work / "catalog.sqlite")
    schema(db)
    inputs = [("wikidata", config["wikidata"])] + list(config["wikipedia"].items())
    inputs += [(name, config[name]) for name in ("wikidata_pages", "wikidata_redirects") if name in config]
    inputs += [("langlinks:" + language, source) for language, source in config.get("langlinks", {}).items()]
    origins = []
    for name, source in inputs:
        sql = name.startswith("wikidata_") or name.startswith("langlinks:")
        if name != "wikidata" and name not in LANGUAGES and not sql:
            raise ValueError(f"unsupported snapshot language: {name}")
        checked_at = source["date"] + "T00:00:00Z"
        datetime.fromisoformat(checked_at.replace("Z", "+00:00"))
        path, digest = download(source, work)
        previous = db.execute("SELECT sha256,complete,records FROM imports WHERE id=?", (name,)).fetchone()
        if previous and previous[0] != digest:
            raise ValueError("snapshot input changed; use a new preparation directory")
        if not previous or not previous[1]:
            db.execute("INSERT OR REPLACE INTO imports VALUES(?,?,0,0)", (name, digest))
            db.commit()
            count = 0
            with records(path) as stream:
                if sql:
                    for count in bulk_sql.import_table(db, stream, name, LANGUAGES):
                        checkpoint()
                        if shutil.disk_usage(work).free < RESERVE:
                            raise ValueError("identity import reached the disk reserve")
                for raw in ([] if sql else rows(stream)):
                    try:
                        values = [entity(raw, checked_at)] if name == "wikidata" else article(raw, name, checked_at)
                    except ValueError as error:
                        if name == "wikidata" or not isinstance(raw.get("name"), str):
                            raise
                        aliases = [raw["name"], *[item["name"] for item in raw.get("redirects") or [] if item.get("name")]]
                        for title in aliases:
                            for kind, key in [("article", f"{name}:{title}"), ("link", f"wikipedia:{name}:{title}")]:
                                db.execute("INSERT OR REPLACE INTO rejected VALUES(?,?,?,?)",
                                           (kind, key, (raw.get("version") or {}).get("identifier") or 0, str(error)))
                        values = []
                    for value in values:
                        if value:
                            put(db, value)
                    if name == "wikidata" and raw.get("type") == "item":
                        for wiki, site in raw.get("sitelinks", {}).items():
                            if wiki.endswith("wiki"):
                                db.execute("INSERT OR REPLACE INTO sitelinks VALUES(?,?,?)", (wiki.removesuffix("wiki"), site["title"], raw["id"]))
                    count += 1
                    if count % 10000 == 0:
                        db.commit()
                        checkpoint()
                        print(f"{name}: {count} records", flush=True)
                        if shutil.disk_usage(work).free < RESERVE:
                            raise ValueError("snapshot import reached the disk reserve")
            db.execute("UPDATE imports SET complete=1,records=? WHERE id=?", (count, name))
            db.commit()
        origins.append(dict(source=name, date=source["date"], sha256=digest, bytes=path.stat().st_size,
                            url=source.get("url"), records=db.execute("SELECT records FROM imports WHERE id=?", (name,)).fetchone()[0]))
    roots = dict(entities=config.get("entities") or [], links=config.get("links") or [], peaks=[], summits=[])
    for path in config.get("osm") or []:
        origins.append(dict(source="osm", sha256=sha(Path(path)), bytes=Path(path).stat().st_size))
        command = ["osmium", "tags-filter", path, "nwr/wikidata", "nwr/wikipedia", "-R", "-f", "opl"]
        with subprocess.Popen(command, stdout=subprocess.PIPE, text=True) as process:
            for line in process.stdout:
                fields = line.rstrip().split(" ")
                tagged = next((field[1:] for field in fields if field.startswith("T")), "")
                decode = lambda text: re.sub(r"%([0-9a-fA-F]+)%", lambda match: chr(int(match[1], 16)), text)
                tags = {decode(key): decode(value) for key, value in (part.split("=", 1) for part in tagged.split(",") if "=" in part)}
                qid = tags.get("wikidata")
                if qid and re.fullmatch(r"Q[1-9][0-9]*", qid):
                    roots["entities"].append(qid)
                link = tags.get("wikipedia")
                if link and ":" in link and re.fullmatch(r"[a-z][a-z0-9-]{1,14}", link.split(":", 1)[0]):
                    roots["links"].append("wikipedia:" + link)
                if tags.get("natural") == "peak":
                    roots["peaks"].extend(([qid] if qid else []) + (["wikipedia:" + link] if link else []))
                    if qid:
                        roots["links"].append("wikidata:" + qid)
                    if fields[0].startswith("n"):
                        coordinate = lambda prefix: float(next(field[1:] for field in fields if field.startswith(prefix)))
                        roots["summits"].append(dict(node_id=int(fields[0][1:]), latitude=coordinate("y"), longitude=coordinate("x"), tags=tags))
            if process.wait():
                raise ValueError("OSM content discovery failed")
    roots = {kind: sorted(set(values)) if kind != "summits" else values for kind, values in roots.items()}
    if not roots["entities"] and not roots["links"]:
        raise ValueError("content preparation needs OSM inputs or explicit identities")
    write_json(work / "roots.json", roots)
    write_json(work / "origins.json", origins)
    db.close()


class SnapshotAcquisition(Acquisition):
    """Entity and article resolution is exclusively from the pinned archive index."""
    def __init__(self, *args, catalog, allow_media=False, **kwargs):
        super().__init__(*args, **kwargs)
        self.db = connect(catalog)
        self.allow_media = allow_media
        for kind, key in self.records:
            value = self.value(kind, key)
            if asset := value.get("asset"):
                row = self.db.execute("SELECT path FROM assets WHERE sha256=?", (asset["sha256"],)).fetchone()
                if row:
                    self.assets[asset["sha256"]] = Path(row[0])

    def load(self, kind, keys):
        for key in sorted(set(keys)):
            value = fact(self.db, kind, key) or fact(self.db, kind, key.replace("_", " "))
            try:
                if kind == "entity" and value is None:
                    value = bulk_sql.redirect(self.db, key, fact)
                if kind == "link" and value is None and key.startswith("wikipedia:"):
                    language, title = key.removeprefix("wikipedia:").split(":", 1)
                    site = self.db.execute("SELECT qid FROM sitelinks WHERE language=? AND title IN (?,?)", (language, title, title.replace("_", " "))).fetchone()
                    if site:
                        item = fact(self.db, "entity", site[0])
                        value = dict(kind="link", key=key, status="present", identity=site[0], revision=item["revision"], checked_at=item["checked_at"],
                                     proof=dict(snapshot=True, bulk_identity=True, item_sitelink=dict(language=language,title=title,qid=site[0])))
                if kind == "link" and value and value["status"] == "present":
                    value = bulk_sql.link(self.db, value, fact, LANGUAGES)
            except ValueError as error:
                self.fail(kind, key, str(error))
                continue
            rejected = self.db.execute("SELECT revision,reason FROM rejected WHERE kind=? AND key=?", (kind, key)).fetchone()
            if not rejected:
                rejected = self.db.execute("SELECT revision,reason FROM rejected WHERE kind=? AND key=?", (kind, key.replace("_", " "))).fetchone()
            if rejected and (not value or rejected[0] >= value.get("revision", 0)):
                self.fail(kind, key, rejected[1])
                continue
            if value is None:
                sources = ["wikidata", "wikidata_pages", "wikidata_redirects"] if kind == "entity" else [key.removeprefix("wikipedia:").split(":", 1)[0]] if kind in {"article", "link"} else []
                proofs = [self.db.execute("SELECT sha256,complete FROM imports WHERE id=?", (source,)).fetchone() for source in sources]
                if sources and all(proof and proof[1] for proof in proofs):
                    self.keep(dict(kind=kind, key=key, status="missing", checked_at=datetime.now(timezone.utc).isoformat(),
                                   proof=dict(requested=key, complete_snapshots={source:proof[0] for source, proof in zip(sources, proofs)})))
                else:
                    self.fail(kind, key, "content-snapshot-coverage-gap")
            else:
                if value["key"] != key:
                    value = dict(value, key=key)
                    alias = {"from": key.split(":", 2)[-1] if kind == "link" else key.split(":", 1)[-1],
                             "to": value.get("title", value.get("proof", {}).get("title"))}
                    if kind == "article":
                        value["aliases"] = [alias, *value.get("aliases", [])]
                    elif kind == "link":
                        value["proof"]["aliases"] = [alias, *value["proof"].get("aliases", [])]
                if value.get("asset"):
                    asset = value["asset"]
                    row = self.db.execute("SELECT path FROM assets WHERE sha256=?", (asset["sha256"],)).fetchone()
                    if not row or not Path(row[0]).is_file():
                        self.fail(kind, key, "content-snapshot-image-unavailable")
                        continue
                    self.assets[asset["sha256"]] = Path(row[0])
                self.keep(value)

    def entities(self, ids, refresh=False):
        self.load("entity", ids)

    def articles(self, requests, refresh=False):
        self.load("article", [f"{item['language']}:{item['title']}" for item in requests])

    def links(self, links, refresh=False):
        qids = [key.removeprefix("wikidata:") for key in links if key.startswith("wikidata:")]
        self.entities(qids)
        for qid in qids:
            value = self.value("entity", qid)
            if value:
                checked_at = self.records[("entity", qid)]["checked_at"]
                if value["status"] == "missing":
                    self.keep(dict(kind="link", key="wikidata:" + qid, status="missing", checked_at=checked_at, proof=value["proof"]))
                else:
                    self.keep(dict(kind="link", key="wikidata:" + qid, status="present", identity=value["identity"],
                                   revision=value["revision"], checked_at=checked_at, proof=value["entity"].get("redirects", {})))
        self.load("link", [key for key in links if key.startswith("wikipedia:")])
        return sorted({record["identity"] for (kind, _), record in self.records.items() if kind == "link" and record["status"] == "present"})

    def commons(self, filenames, refresh=False):
        if self.allow_media:
            for name in filenames:
                key = "File:" + name.removeprefix("File:").replace("_", " ")
                if fact(self.db, "commons", key):
                    self.load("commons", [key])
                    value = fact(self.db, "commons", key)
                    media = f"M{value.get('pageid')}"
                    if fact(self.db, "mediainfo", media):
                        self.load("mediainfo", [media])
            return super().commons(filenames, refresh)
        self.load("commons", ["File:" + name.removeprefix("File:").replace("_", " ") for name in filenames])

    def categories(self, names, refresh=False):
        if self.allow_media:
            for name in names:
                key = "Category:" + name.removeprefix("Category:").replace("_", " ")
                if fact(self.db, "category", key):
                    self.load("category", [key])
            return super().categories(names, refresh)
        self.load("category", ["Category:" + name.removeprefix("Category:").replace("_", " ") for name in names])


def retain(catalog, manifest, out):
    db = connect(catalog)
    for pin in manifest["records"]:
        path = out / pin["path"]
        if sha(path) != pin["sha256"]:
            raise ValueError("staged content digest changed")
        value = json.loads(path.read_bytes())
        value["checked_at"] = pin["checked_at"]
        put(db, value)
        db.execute("INSERT OR IGNORE INTO kept VALUES(?,?)", (pin["kind"], pin["key"]))
    for asset in manifest.get("assets", []):
        path = (out / asset["path"]).resolve()
        if sha(path) != asset["sha256"] or path.stat().st_size != asset["bytes"]:
            raise ValueError("staged image digest changed")
        target = catalog.parent / "media" / asset["sha256"]
        target.parent.mkdir(parents=True, exist_ok=True)
        if not target.exists():
            os.link(path, target)
        db.execute("INSERT OR REPLACE INTO assets VALUES(?,?,?)", (asset["sha256"], str(target.resolve()), asset["bytes"]))
    db.commit()
    db.close()


def export(work, out):
    """Bounded bundles and a compact SQLite lookup index; raw projections stay local."""
    out.mkdir(parents=True, exist_ok=True)
    db = connect(work / "catalog.sqlite")
    index_path = out / "index.sqlite"
    index_path.unlink(missing_ok=True)
    index = connect(index_path)
    index.execute("CREATE TABLE lookup(kind TEXT,key TEXT,bundle TEXT,PRIMARY KEY(kind,key)) WITHOUT ROWID")
    files = []
    entries, size = [], 32

    def flush():
        nonlocal entries, size
        if not entries:
            return
        data = encoded(dict(schema=1, records=[json.loads(entry) for _, _, entry in entries]))
        digest = hashlib.sha256(data).hexdigest()
        (out / digest).write_bytes(data)
        files.append(dict(name=digest, sha256=digest, bytes=len(data), kind="bundle"))
        index.executemany("INSERT INTO lookup VALUES(?,?,?)", [(kind, key, digest) for kind, key, _ in entries])
        entries, size = [], 32

    for kind, key, data in db.execute("SELECT f.kind,f.key,f.data FROM facts f JOIN kept k USING(kind,key) ORDER BY f.kind,f.key"):
        if size + len(data) + 1 > BOUND:
            flush()
        if len(data) + 32 > BOUND:
            raise ValueError("fact exceeds bundle bound")
        entries.append((kind, key, data))
        size += len(data) + 1
    flush()
    index.commit()
    count = index.execute("SELECT COUNT(*) FROM lookup").fetchone()[0]
    index.close()
    files.append(dict(name="index.sqlite", sha256=sha(index_path), bytes=index_path.stat().st_size, kind="index"))
    for digest, path, length in db.execute("SELECT sha256,path,bytes FROM assets"):
        target = out / digest
        if not target.exists():
            os.link(path, target)
        if sha(target) != digest or target.stat().st_size != length:
            raise ValueError("prepared image changed before publication")
        files.append(dict(name=digest, sha256=digest, bytes=length, kind="image"))
    roots = json.loads((work / "roots.json").read_bytes())
    manifest = dict(schema=1, complete=True, origins=json.loads((work / "origins.json").read_bytes()),
                    coverage={key: len(values) for key, values in roots.items()}, coverage_sha256=sha(work / "roots.json"),
                    records=count, files=files)
    write_json(out / "manifest.json", manifest)
    db.close()


def hydrate(index_path, files, catalog, request):
    """Locate only the bundles needed by this request and its dependency closure."""
    index = connect(index_path)
    db = connect(catalog)
    schema(db)
    loaded, missing = set(), set()

    def load(kind, key):
        row = index.execute("SELECT bundle FROM lookup WHERE kind=? AND key=?", (kind, key)).fetchone()
        if not row:
            row = index.execute("SELECT bundle FROM lookup WHERE kind=? AND key=?", (kind, key.replace("_", " "))).fetchone()
        if not row:
            return
        digest = row[0]
        if digest in loaded:
            return
        path = Path(files.get(digest, ""))
        if not path.is_file():
            missing.add(digest)
            return
        if sha(path) != digest:
            raise ValueError("published content bundle checksum changed")
        raw = path.read_bytes()
        if len(raw) > BOUND:
            raise ValueError("published content bundle exceeds bound")
        for value in json.loads(raw)["records"]:
            put(db, value)
        loaded.add(digest)

    for name, kind in [("entities", "entity"), ("links", "link"), ("commons", "commons"), ("categories", "category"), ("files", "file")]:
        for key in request.get(name, []):
            key = ("File:" + key.removeprefix("File:").replace("_", " ")) if kind in ("commons", "file") else key
            key = ("Category:" + key.removeprefix("Category:").replace("_", " ")) if kind == "category" else key
            load(kind, key)
    for item in request.get("articles", []):
        load("article", f"{item['language']}:{item['title']}")
    pending = set(request.get("entities", [])) | {item.removeprefix("wikidata:") for item in request.get("links", []) if item.startswith("wikidata:")}
    for link in request.get("links", []):
        value = fact(db, "link", link)
        if value and value["identity"].startswith("Q"):
            pending.add(value["identity"])
    class Dependencies:
        def entities(self, ids, refresh=False):
            for qid in ids:
                load("entity", qid)
        def value(self, kind, key):
            return fact(db, kind, key)
    Acquisition.dependencies(Dependencies(), sorted(pending))
    # MediaInfo provides the existing photo selector's subject evidence.
    for filename in request.get("commons", []):
        value = fact(db, "commons", "File:" + filename.removeprefix("File:").replace("_", " "))
        if value and value.get("pageid"):
            load("mediainfo", f"M{value['pageid']}")
    for filename in request.get("files", []):
        value = fact(db, "file", "File:" + filename.removeprefix("File:").replace("_", " "))
        if value and value.get("asset"):
            asset = value["asset"]
            path = Path(files.get(asset["sha256"], ""))
            if not path.is_file():
                missing.add(asset["sha256"])
            elif sha(path) != asset["sha256"] or path.stat().st_size != asset["bytes"]:
                raise ValueError("published image checksum changed")
            else:
                db.execute("INSERT OR REPLACE INTO assets VALUES(?,?,?)", (asset["sha256"], str(path.resolve()), asset["bytes"]))
    db.commit()
    db.close()
    index.close()
    return sorted(missing)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["import", "retain", "export", "hydrate"])
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--config", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--index", type=Path)
    parser.add_argument("--files", type=Path)
    parser.add_argument("--requests", type=Path)
    args = parser.parse_args()
    if args.action == "import":
        import_sources(json.loads(args.config.read_bytes()), args.work)
    elif args.action == "retain":
        retain(args.work / "catalog.sqlite", json.loads(args.manifest.read_bytes()), args.out)
    elif args.action == "export":
        export(args.work, args.out)
    else:
        missing = hydrate(args.index, json.loads(args.files.read_bytes()), args.work / "catalog.sqlite", json.loads(args.requests.read_bytes()))
        write_json(args.out, dict(missing=missing))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
