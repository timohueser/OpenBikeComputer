"""Bulk identity proofs from Wikimedia's compressed SQL table dumps."""
from __future__ import annotations

import re

TOKEN = re.compile(r"\s*('(?:[^'\\]|\\.)*'|NULL|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[(),;])")
ESCAPES = {"0": "\0", "n": "\n", "r": "\r", "t": "\t", "b": "\b", "Z": "\x1a"}


def tuples(stream, table):
    prefix = f"INSERT INTO `{table}` VALUES"
    active, declared = False, False
    for raw in stream:
        line = raw.decode("utf-8")
        if line.startswith(f"CREATE TABLE `{table}`"):
            declared = True
        if line.startswith(prefix):
            active, declared = True, True
            line = line[len(prefix):]
        elif not active:
            continue
        position, row = 0, None
        while position < len(line.rstrip()):
            token = TOKEN.match(line, position)
            if not token:
                raise ValueError(f"invalid {table} dump tuple")
            position = token.end()
            value = token[1]
            if value == "(":
                if row is not None:
                    raise ValueError("nested dump tuple")
                row = []
            elif value == ")":
                if row is None:
                    raise ValueError("unmatched dump tuple")
                yield row
                row = None
            elif value == ";":
                active = False
            elif value != ",":
                if row is None:
                    raise ValueError("dump value outside tuple")
                if value.startswith("'"):
                    value = re.sub(r"\\(.)", lambda match: ESCAPES.get(match[1], match[1]), value[1:-1])
                elif value == "NULL":
                    value = None
                else:
                    value = float(value) if any(char in value for char in ".eE") else int(value)
                row.append(value)
        if row is not None:
            raise ValueError("incomplete dump tuple")
    if active or not declared:
        raise ValueError(f"incomplete or absent {table} SQL table")


def schema(db):
    db.executescript("""
        CREATE TABLE IF NOT EXISTS wd_pages(pageid INTEGER PRIMARY KEY,qid TEXT UNIQUE);
        CREATE TABLE IF NOT EXISTS wd_redirects(qid TEXT PRIMARY KEY,target TEXT);
        CREATE TABLE IF NOT EXISTS langlinks(language TEXT,pageid INTEGER,code TEXT,title TEXT,
            PRIMARY KEY(language,pageid,code)) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS sitelinks(language TEXT,title TEXT,qid TEXT,
            PRIMARY KEY(language,title)) WITHOUT ROWID;
    """)


def import_table(db, stream, name, languages):
    table = "page" if name == "wikidata_pages" else "redirect" if name == "wikidata_redirects" else "langlinks"
    count = 0
    for row in tuples(stream, table):
        if table == "page" and row[1] == 0 and re.fullmatch(r"Q[1-9][0-9]*", row[2]):
            db.execute("INSERT OR REPLACE INTO wd_pages VALUES(?,?)", (row[0], row[2]))
        elif table == "redirect" and row[1] == 0 and not row[3] and re.fullmatch(r"Q[1-9][0-9]*", row[2]):
            source = db.execute("SELECT qid FROM wd_pages WHERE pageid=?", (row[0],)).fetchone()
            if not source:
                raise ValueError("Wikidata redirect source absent from page dump")
            db.execute("INSERT OR REPLACE INTO wd_redirects VALUES(?,?)", (source[0], row[2]))
        elif table == "langlinks" and row[1] in languages:
            db.execute("INSERT OR REPLACE INTO langlinks VALUES(?,?,?,?)", (name.removeprefix("langlinks:"), *row))
        count += 1
        if count % 10000 == 0:
            db.commit()
            yield count
    yield count


def redirect(db, qid, fact):
    target, seen = qid, []
    while row := db.execute("SELECT target FROM wd_redirects WHERE qid=?", (target,)).fetchone():
        if target in seen or len(seen) >= 64:
            raise ValueError("Wikidata redirect chain is cyclic or too long")
        seen.append(target)
        target = row[0]
    value = fact(db, "entity", target)
    if value and target != qid:
        value = dict(value, key=qid, entity=dict(value["entity"], redirects={"from": qid, "to": target}))
        value["redirect_proof"] = dict(chain=seen + [target], source="wikidata_redirects")
    return value


def link(db, value, fact, languages):
    if value["proof"].get("bulk_identity"):
        return value
    language, title = value["proof"]["language"], value["proof"]["title"]
    item = db.execute("SELECT qid FROM sitelinks WHERE language=? AND title=?", (language, title)).fetchone()
    if item or value["identity"].startswith("Q"):
        return dict(value, identity=item[0] if item else value["identity"], proof=dict(value["proof"], bulk_identity=True))
    complete = db.execute("SELECT complete FROM imports WHERE id=?", ("langlinks:" + language,)).fetchone()
    if not complete or not complete[0]:
        raise ValueError(f"no-item identity requires the {language} langlinks SQL snapshot")
    links = {language: title, **dict(db.execute("SELECT code,title FROM langlinks WHERE language=? AND pageid=?", (language, value["proof"]["pageid"])))}
    selected = next(code for code in languages if code in links)
    page = fact(db, "article", f"{selected}:{links[selected]}") or fact(db, "article", f"{selected}:{links[selected].replace('_', ' ')}")
    if not page:
        raise ValueError("canonical language-link target absent from article snapshot")
    item = db.execute("SELECT qid FROM sitelinks WHERE language=? AND title=?", (selected, page["title"])).fetchone()
    identity = page.get("qid") or (item[0] if item else None) or f"wiki-{selected}-{page['pageid']}"
    return dict(value, identity=identity,
        proof=dict(value["proof"], bulk_identity=True, langlinks=links,
                   canonical=dict(language=selected, title=page["title"], pageid=page["pageid"], aliases=page["aliases"])),
        sitelinks={code + "wiki": {"title": name} for code, name in links.items()})
