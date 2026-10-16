"""The work catalog: a resumable import of the Wikimedia dumps into compact facts.

Wikidata pass 1 reads every item once. It keeps the compact entity of each subject, the P279 and
P131 edges of every item, and the item sitelinks that OSM `wikipedia` tags name. The class,
country and locale closure of the subjects follows from those edges, and pass 2 keeps the compact
entities of that closure. The article import keeps only the pages that a selected subject or an
OSM tag names.
"""
from __future__ import annotations

from functools import lru_cache
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import shutil
import sqlite3
import time
from urllib.parse import urlencode

from ..landmark_capture import LANGUAGES, MAX_JSON_SOURCE, MAX_LOCALE_DEPTH, MAX_LOCALES
from . import dumps
from .facts import compact_entity, encoded, lead_html, original_notices

QID = re.compile(r"Q[1-9][0-9]*")
# The id leads each Wikidata dump line; a sitelink title is a JSON string.
ITEM = re.compile(rb'"id":"(Q[1-9][0-9]*)"')
SITELINK = re.compile(rb'"site":"([^"]+)","title":"((?:[^"\\]|\\.)*)"')
NAME = re.compile(rb'"name":"((?:[^"\\]|\\.)*)"')
SCHEMA = """
CREATE TABLE IF NOT EXISTS imports(id TEXT PRIMARY KEY, sha256 TEXT, complete INTEGER);
-- One row per shard cell of a subject; cell '' is the common shard.
CREATE TABLE IF NOT EXISTS subjects(osm TEXT, cell TEXT, wikidata TEXT, wikipedia TEXT, peak INTEGER, PRIMARY KEY(osm,cell)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS facts(kind TEXT, key TEXT, revision INTEGER, data BLOB, PRIMARY KEY(kind,key)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS failures(kind TEXT, key TEXT, reason TEXT, PRIMARY KEY(kind,key)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS edges(prop INTEGER, qid TEXT, target TEXT, PRIMARY KEY(prop,qid,target)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS pages(pageid INTEGER PRIMARY KEY, qid TEXT);
CREATE TABLE IF NOT EXISTS redirects(qid TEXT PRIMARY KEY, target TEXT) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS langlinks(language TEXT, pageid INTEGER, code TEXT, title TEXT,
    PRIMARY KEY(language,pageid,code)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS wanted(language TEXT, title TEXT, article INTEGER, PRIMARY KEY(language,title)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS titles(language TEXT, title TEXT, pageid INTEGER, qid TEXT, canonical TEXT, revision INTEGER,
    PRIMARY KEY(language,title)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS uses(subject TEXT, kind TEXT, key TEXT, PRIMARY KEY(subject,kind,key)) WITHOUT ROWID;
"""


def connect(work: Path):
    work.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(work / "catalog.sqlite")
    db.execute("PRAGMA cache_size=-262144")
    db.execute("PRAGMA temp_store=FILE")
    db.executescript(SCHEMA)
    return db


def put(db, value: dict):
    """Keep one compact fact. A record of a newer revision replaces an older one, never the reverse."""
    data = encoded(value)
    if len(data) > MAX_JSON_SOURCE:
        return fail(db, value["kind"], value["key"], "compact-content-exceeds-bound")
    revision = value.get("revision") if isinstance(value.get("revision"), int) else 0
    db.execute("""INSERT INTO facts VALUES(?,?,?,?) ON CONFLICT(kind,key) DO UPDATE SET
        revision=excluded.revision,data=excluded.data WHERE excluded.revision >= facts.revision""",
               (value["kind"], value["key"], revision, data))
    db.execute("DELETE FROM failures WHERE kind=? AND key=?", (value["kind"], value["key"]))


def fact(db, kind: str, key: str) -> dict | None:
    row = db.execute("SELECT data FROM facts WHERE kind=? AND key=?", (kind, key)).fetchone()
    return json.loads(row[0]) if row else None


def fail(db, kind: str, key: str, reason: str):
    """Unresolved work: never a confirmed missing fact, and never in place of a kept fact."""
    if not db.execute("SELECT 1 FROM facts WHERE kind=? AND key=?", (kind, key)).fetchone():
        db.execute("INSERT OR REPLACE INTO failures VALUES(?,?,?)", (kind, key, reason))


def failed(db, kind: str, key: str) -> bool:
    return db.execute("SELECT 1 FROM failures WHERE kind=? AND key=?", (kind, key)).fetchone() is not None


def complete(db, name: str) -> str | None:
    """The SHA-256 of the input that import `name` finished with."""
    row = db.execute("SELECT sha256 FROM imports WHERE id=? AND complete=1", (name,)).fetchone()
    return row[0] if row else None


def scan(db, work: Path, name: str, digest: str, rows, record, done=lambda: None, source: Path | None = None):
    """Run import `name` over `rows` once. A finished import with the same input is not repeated;
    an interrupted one starts again; a changed input never resumes. The progress of an import of
    the dump `source` tells the share of it read and the time left."""
    previous = db.execute("SELECT sha256,complete FROM imports WHERE id=?", (name,)).fetchone()
    if previous and previous[0] != digest:
        raise ValueError(f"{name}: input changed; use a new work directory")
    if previous and previous[1]:
        return
    db.execute("INSERT OR REPLACE INTO imports VALUES(?,?,0)", (name, digest))
    db.commit()
    count, start = 0, time.monotonic()
    for count, row in enumerate(rows, 1):
        record(row)
        if count % 100000 == 0:
            db.commit()
            print(f"{name}: {count} records{progress(start, source and dumps.share(source))}", flush=True)
            if shutil.disk_usage(work).free < dumps.RESERVE:
                raise ValueError(f"{name}: import reached the disk reserve")
    done()
    db.execute("UPDATE imports SET complete=1 WHERE id=?", (name,))
    db.commit()


def progress(start: float, share: float | None) -> str:
    """The share done of a task that started at `start`, its time so far, and the estimated rest."""
    elapsed = time.monotonic() - start
    if not share:
        return f", {span(elapsed)}"
    share = min(share, 1)
    return f", {share:.0%} done, {span(elapsed)}, about {span(elapsed * (1 - share) / share)} left"


def span(seconds: float) -> str:
    minutes = round(seconds / 60)
    return f"{minutes // 60} h {minutes % 60} min" if minutes >= 60 else f"{minutes} min"


def parsed(path: Path, keep):
    """The JSON records of `path` whose raw line `keep` accepts; the others are never decoded."""
    for line in dumps.lines(path):
        if keep(line):
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"{path.name}: a record is not an object")
            yield value
        else:
            yield None


def text(raw: bytes) -> str:
    return json.loads(b'"' + raw + b'"') if b"\\" in raw else raw.decode()


def chain(db, qid: str) -> list[str] | None:
    """`qid` and the Wikidata redirects that it follows to its canonical item. A cyclic or too
    long chain is a failure of `qid` alone, never of the import."""
    seen = [qid]
    while row := db.execute("SELECT target FROM redirects WHERE qid=?", (seen[-1],)).fetchone():
        if row[0] in seen or len(seen) > 64:
            fail(db, "entity", qid, "Wikidata redirect chain is cyclic or too long")
            return None
        seen.append(row[0])
    return seen


def canonical(db, qid: str) -> str | None:
    path = chain(db, qid)
    return path and path[-1]


def graph(db):
    """The targets of one property of an item, after redirects, from the pass 1 edges."""
    @lru_cache(maxsize=1 << 20)
    def targets(prop: int, qid: str) -> frozenset[str]:
        return frozenset(row[0] for row in db.execute("SELECT target FROM edges WHERE prop=? AND qid=?", (prop, canonical(db, qid))))
    return targets


def classes(db, targets) -> set[str]:
    """Every P31 type of a subject, and its P279 ancestors."""
    found = set()
    pending = {row[0] for row in db.execute("SELECT DISTINCT target FROM edges WHERE prop=31")}
    while pending:
        found |= pending
        pending = set().union(*(targets(279, qid) for qid in pending)) - found
    return found


def locales(targets, qid: str) -> set[str]:
    """The countries and the bounded P131 traversal of one subject, in the order of the API acquisition."""
    visited, frontier = set(), set(targets(131, qid))
    for _ in range(MAX_LOCALE_DEPTH):
        frontier = set(sorted(frontier - visited)[:MAX_LOCALES - len(visited)])
        if not frontier:
            break
        visited |= frontier
        frontier = set().union(*(targets(131, locale) for locale in frontier)) - visited
    return visited | targets(17, qid)


def subject_items(db) -> set[str]:
    """The canonical items of the OSM `wikidata` tags, and the items whose sitelinks the
    `wikipedia` tags name."""
    found = {canonical(db, row[0]) for row in db.execute("SELECT DISTINCT wikidata FROM subjects WHERE wikidata IS NOT NULL")} - {None}
    for (data,) in db.execute("SELECT data FROM facts WHERE kind='link' AND key LIKE 'wikipedia:%'"):
        value = json.loads(data)
        if value["status"] == "present" and QID.fullmatch(value["identity"]):
            found.add(value["identity"])
    return found


def candidates(db, out: Path):
    """What the landmark rule of the compilers reads, for the producer: `{"class": fact}` for the
    entity of each class, then `{"subject": fact}` for the entity of each OSM `wikidata` tag, and
    `{"peak": identity, "links": [key], "entity": fact}` for each identity that the links of peaks
    resolve to, with those link keys."""
    if not complete(db, "wikidata:2"):
        raise ValueError("the Wikidata import has not finished; run the import step first")
    partial = out.with_suffix(".partial")
    with partial.open("w") as output:
        for qid in sorted(classes(db, graph(db))):
            if value := fact(db, "entity", qid):
                output.write(json.dumps({"class": value}, ensure_ascii=False) + "\n")
        tags = db.execute("SELECT DISTINCT wikidata FROM subjects WHERE wikidata IS NOT NULL").fetchall()
        for qid in sorted({canonical(db, tag) for (tag,) in tags} - {None}):
            if value := fact(db, "entity", qid):
                output.write(json.dumps({"subject": value}, ensure_ascii=False) + "\n")
        peaks = {}
        for wikidata, wikipedia in db.execute("SELECT DISTINCT wikidata, wikipedia FROM subjects WHERE peak=1"):
            for key in (f"wikidata:{wikidata}" if wikidata else None, f"wikipedia:{wikipedia}" if wikipedia else None):
                if key and (link := fact(db, "link", key)) and link["status"] == "present":
                    peaks.setdefault(link["identity"], set()).add(key)
        output.writelines(json.dumps({"peak": identity, "links": sorted(links), "entity": fact(db, "entity", identity)},
                                     ensure_ascii=False) + "\n" for identity, links in sorted(peaks.items()))
    partial.replace(out)


def entity(raw: dict) -> dict:
    return dict(kind="entity", key=raw["id"], status="present", identity=raw["id"], revision=raw["lastrevid"],
                entity=compact_entity(raw))


def claimed(raw: dict, prop: str) -> set[str]:
    values = (claim.get("mainsnak", {}).get("datavalue", {}).get("value") for claim in raw.get("claims", {}).get(prop, []))
    return {value["id"] for value in values if isinstance(value, dict) and QID.fullmatch(value.get("id") or "")}


def wikidata(db, work: Path, path: Path, digest: str):
    """Both passes over the Wikidata JSON dump, then the entity and `wikidata:` link facts of
    every subject and of its closure."""
    tagged = {row[0]: canonical(db, row[0]) for row in db.execute("SELECT DISTINCT wikidata FROM subjects WHERE wikidata IS NOT NULL")}
    items = set(tagged.values()) - {None}
    wanted = {}
    for (tag,) in db.execute("SELECT DISTINCT wikipedia FROM subjects WHERE wikipedia IS NOT NULL"):
        language, title = tag.split(":", 1)
        wanted.setdefault((language.replace("-", "_") + "wiki", title.replace("_", " ")), []).append(tag)

    def first(line: bytes) -> bool:
        item = ITEM.search(line, 0, 256)
        return ((item and item[1].decode() in items) or b'"P279"' in line or b'"P131"' in line
                or bool(wanted) and any((site.decode(), text(title)) in wanted for site, title in SITELINK.findall(line)))

    def keep(raw: dict | None):
        if raw is None or raw.get("type") != "item" or not QID.fullmatch(raw.get("id", "")):
            return
        qid = raw["id"]
        for site, link in raw.get("sitelinks", {}).items():
            for tag in wanted.get((site, link["title"]), ()):
                items.add(qid)
                put(db, dict(kind="link", key="wikipedia:" + tag, status="present", identity=qid, revision=raw["lastrevid"],
                             proof=dict(snapshot=True, item_sitelink=dict(site=site, title=link["title"], qid=qid))))
        subject = qid in items
        db.executemany("INSERT OR IGNORE INTO edges VALUES(?,?,?)",
                       [(prop, qid, target) for prop in ((31, 17, 131, 279) if subject else (131, 279)) for target in claimed(raw, f"P{prop}")])
        if subject:
            put(db, entity(raw))

    scan(db, work, "wikidata:1", digest, parsed(path, first), keep, source=path)
    if complete(db, "wikidata:2"):
        return
    targets = graph(db)
    found = subject_items(db)
    closure = classes(db, targets)
    for qid in found:
        closure |= locales(targets, qid)
    needed = {canonical(db, qid) for qid in closure} - found - {None}

    def second(raw: dict | None):
        if raw is not None:
            put(db, entity(raw))

    def done():
        proof = dict(complete_snapshots={name: complete(db, name) for name in ("wikidata:1", "wikidata_pages", "wikidata_redirects")})
        for qid in sorted(closure | set(tagged) | found):
            path = chain(db, qid)
            if path is None or fact(db, "entity", qid):
                continue
            target = fact(db, "entity", path[-1])
            if len(path) > 1 and target and target["status"] == "present":
                put(db, dict(target, key=qid, entity=dict(target["entity"], redirects={"from": qid, "to": path[-1]}),
                             redirect_proof=dict(chain=path, source="wikidata_redirects")))
            else:
                put(db, dict(kind="entity", key=qid, status="missing", proof=dict(proof, requested=qid)))
        for qid in tagged:
            value = fact(db, "entity", qid)
            if value is None:
                fail(db, "link", "wikidata:" + qid, "Wikidata redirect chain is cyclic or too long")
            elif value["status"] == "missing":
                put(db, dict(kind="link", key="wikidata:" + qid, status="missing", proof=value["proof"]))
            else:
                put(db, dict(kind="link", key="wikidata:" + qid, status="present", identity=value["identity"],
                             revision=value["revision"], proof=value["entity"].get("redirects", {})))

    scan(db, work, "wikidata:2", digest, parsed(path, lambda line: (item := ITEM.search(line, 0, 256)) is not None
                                                  and item[1].decode() in needed), second, done, source=path)


def sql(db, work: Path, name: str, path: Path, digest: str):
    """`wikidata_pages` and `wikidata_redirects` give the item redirects; `langlinks:<language>`
    gives the supported language links of one Wikipedia."""
    table, columns = {"wikidata_pages": ("page", ("page_id", "page_namespace", "page_title", "page_is_redirect")),
                      "wikidata_redirects": ("redirect", ("rd_from", "rd_namespace", "rd_title", "rd_interwiki"))
                      }.get(name, ("langlinks", ("ll_from", "ll_lang", "ll_title")))

    def record(row):
        if table == "page" and row[1] == 0 and row[3] == 1 and QID.fullmatch(row[2]):
            db.execute("INSERT OR REPLACE INTO pages VALUES(?,?)", (row[0], row[2]))
        elif table == "redirect" and row[1] == 0 and not row[3] and QID.fullmatch(row[2]):
            source = db.execute("SELECT qid FROM pages WHERE pageid=?", (row[0],)).fetchone()
            if source:
                db.execute("INSERT OR REPLACE INTO redirects VALUES(?,?)", (source[0], row[2]))
        elif table == "langlinks" and row[1] in LANGUAGES:
            db.execute("INSERT OR REPLACE INTO langlinks VALUES(?,?,?,?)", (name.removeprefix("langlinks:"), *row))

    scan(db, work, name, digest, dumps.tuples(path, table, columns), record, source=path)


class Proof(HTMLParser):
    about = None
    pageid = None

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "html":
            self.about = attrs.get("about")
        if tag == "meta" and attrs.get("property") == "mw:pageId":
            self.pageid = attrs.get("content")


def article(raw: dict, language: str) -> dict:
    """The article fact of one Enterprise record. The revision is the one that the rendered HTML
    proves, never the one that the record declares."""
    title, pageid = raw.get("name"), raw.get("identifier")
    declared = (raw.get("version") or {}).get("identifier")
    if not isinstance(title, str) or type(pageid) is not int or pageid <= 0:
        raise ValueError("invalid snapshot page identity")
    html = (raw.get("article_body") or {}).get("html")
    licenses = raw.get("license") or []
    if isinstance(licenses, dict):
        licenses = [licenses]
    license = next((item for item in licenses if isinstance(item, dict) and "creativecommons.org/licenses/by-sa/" in (item.get("url") or "")), None)
    if not isinstance(html, str) or not license:
        raise ValueError(f"snapshot article lacks revision-bound HTML or licence: {language}:{title}")
    proof = Proof()
    proof.feed(html.partition("</head>")[0])
    # Newer Parsoid HTML writes a protocol-relative URL and the local name of Special:Redirect,
    # such as `//de.wikipedia.org/wiki/Spezial:Weiterleitung/revision/N`.
    match = re.fullmatch(rf"(?:https:)?//{re.escape(language)}\.wikipedia\.org/wiki/[^/:]+:[^/]+/revision/([1-9][0-9]*)", proof.about or "")
    if not match or proof.pageid != str(pageid):
        raise ValueError("rendered HTML has no matching page and revision proof")
    revision = int(match[1])
    if (raw.get("in_language") or {}).get("identifier") != language:
        raise ValueError("snapshot article language differs from its source")
    qid = (raw.get("main_entity") or {}).get("identifier")
    if qid is not None and not QID.fullmatch(str(qid)):
        raise ValueError("invalid article Wikidata identity")
    return dict(kind="article", key=f"{language}:{title}", status="present", identity=f"{language}:{pageid}", revision=revision,
                language=language, title=title, pageid=pageid, qid=qid, aliases=[],
                timestamp=raw.get("date_modified") if revision == declared else None,
                url=f"https://{language}.wikipedia.org/w/index.php?" + urlencode(dict(title=title, oldid=revision)),
                lead_html=lead_html(html), license=license, original_notices=original_notices(html),
                representation=dict(about=proof.about, pageid=pageid, revision=revision, declared_revision=declared,
                                    html_sha256=hashlib.sha256(html.encode()).hexdigest()))


def want(db, work: Path, selected: list[str], languages=LANGUAGES):
    """The article titles of the selected subjects, and the titles of the `wikipedia` tags that no
    item sitelink names."""
    if not complete(db, "wikidata:2"):
        raise ValueError("the Wikidata import has not finished; run the import step first")
    # The article imports keep only what this selection wants, so it never changes in one work directory.
    scan(db, work, "selected", hashlib.sha256(json.dumps([sorted(set(selected)), sorted(languages)]).encode()).hexdigest(), (), lambda _: None)
    for qid in selected:
        value = fact(db, "entity", qid)
        for language in LANGUAGES if value and value["status"] == "present" else ():
            if link := value["entity"]["sitelinks"].get(language + "wiki"):
                if language not in languages:
                    put(db, dict(kind="article", key=f"{language}:{link['title']}", status="missing", reason="language_not_prepared"))
                    continue
                db.execute("INSERT INTO wanted VALUES(?,?,1) ON CONFLICT(language,title) DO UPDATE SET article=1", (language, link["title"]))
    for (tag,) in db.execute("SELECT DISTINCT wikipedia FROM subjects WHERE wikipedia IS NOT NULL"):
        language, title = tag.split(":", 1)
        if language in languages and not fact(db, "link", "wikipedia:" + tag):
            db.execute("INSERT OR IGNORE INTO wanted VALUES(?,?,0)", (language, title.replace("_", " ")))
    db.commit()


def articles(db, work: Path, language: str, path: Path, digest: str):
    """The wanted pages of one Enterprise HTML snapshot. A page without an item also wants its
    canonical page, which a more preferred language holds: import languages in reverse order."""
    name = "wikipedia:" + language
    wanted = dict(db.execute("SELECT title, article FROM wanted WHERE language=?", (language,)))

    def record(raw: dict | None):
        if raw is None or (raw.get("namespace") or {}).get("identifier", 0) != 0:
            return
        names = [raw.get("name"), *[item.get("name") for item in raw.get("redirects") or []]]
        requested = [title for title in dict.fromkeys(names) if title in wanted]
        try:
            value = article(raw, language)
        except ValueError as error:
            for title in requested:
                fail(db, "article", f"{language}:{title}", str(error))
            return
        for title in requested:
            db.execute("""INSERT INTO titles VALUES(?,?,?,?,?,?) ON CONFLICT(language,title) DO UPDATE SET pageid=excluded.pageid,
                qid=excluded.qid,canonical=excluded.canonical,revision=excluded.revision WHERE excluded.revision >= titles.revision""",
                       (language, title, value["pageid"], value["qid"], value["title"], value["revision"]))
            if wanted[title] or not value["qid"]:
                put(db, dict(value, key=f"{language}:{title}", aliases=aliases(title, value["title"])))
        if requested and not value["qid"]:
            links = dict(db.execute("SELECT code, title FROM langlinks WHERE language=? AND pageid=?", (language, value["pageid"])))
            code = next(code for code in LANGUAGES if code == language or code in links)
            if code != language:
                db.execute("INSERT INTO wanted VALUES(?,?,1) ON CONFLICT(language,title) DO UPDATE SET article=1", (code, links[code].replace("_", " ")))

    def done():
        for title, kept in wanted.items():
            key = f"{language}:{title}"
            if kept and not fact(db, "article", key) and not failed(db, "article", key):
                put(db, dict(kind="article", key=key, status="missing", proof=dict(requested=key, complete_snapshots={name: digest})))

    scan(db, work, name, digest, parsed(path, lambda line: any(text(title) in wanted for title in NAME.findall(line))), record, done, source=path)


def links(db):
    """The `wikipedia:` link fact of each OSM tag that no item sitelink names, from the article
    snapshots. A page without an item takes the identity of its canonical page: the first
    supported language among its language links. Its sitelinks are the requested titles, which
    key the article facts; the aliases map them to the canonical titles."""
    for (tag,) in db.execute("SELECT DISTINCT wikipedia FROM subjects WHERE wikipedia IS NOT NULL"):
        key = "wikipedia:" + tag
        language, title = tag.split(":", 1)
        title = title.replace("_", " ")
        if fact(db, "link", key) or not complete(db, "wikipedia:" + language):
            continue
        page = db.execute("SELECT pageid, qid, canonical, revision FROM titles WHERE language=? AND title=?", (language, title)).fetchone()
        if page is None:
            reason = db.execute("SELECT reason FROM failures WHERE kind='article' AND key=?", (f"{language}:{title}",)).fetchone()
            if reason:
                fail(db, "link", key, reason[0])
            else:
                put(db, dict(kind="link", key=key, status="missing", proof=dict(requested=key, complete_snapshots={"wikipedia:" + language: complete(db, "wikipedia:" + language)})))
            continue
        pageid, identity, title_of_page, revision = page
        proof = dict(language=language, title=title_of_page, pageid=pageid, snapshot=True, aliases=aliases(title, title_of_page))
        sitelinks = {language: title}
        if not identity:
            if not complete(db, "langlinks:" + language):
                fail(db, "link", key, f"no-item identity requires the {language} langlinks SQL snapshot")
                continue
            languages = dict(db.execute("SELECT code, title FROM langlinks WHERE language=? AND pageid=?", (language, pageid)))
            code = next(code for code in LANGUAGES if code == language or code in languages)
            requested = title if code == language else languages[code].replace("_", " ")
            target = db.execute("SELECT pageid, qid, canonical FROM titles WHERE language=? AND title=?", (code, requested)).fetchone()
            if target is None:
                fail(db, "link", key, "canonical language-link target absent from article snapshot")
                continue
            identity = target[1] or f"wiki-{code}-{target[0]}"
            proof.update(langlinks=languages, canonical=dict(language=code, title=target[2], pageid=target[0], aliases=aliases(requested, target[2])))
            sitelinks[code] = requested
        put(db, dict(kind="link", key=key, status="present", identity=identity, revision=revision, proof=proof,
                     sitelinks={code + "wiki": {"title": name} for code, name in sitelinks.items()}))
    db.commit()


def aliases(requested: str, title: str) -> list[dict]:
    return [] if requested == title else [{"from": requested, "to": title}]
