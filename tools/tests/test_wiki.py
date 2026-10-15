from io import BytesIO
import bz2
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.parse import parse_qs, urlparse

from tools.wiki import catalog, commons, dumps, shards, subjects
from tools.wiki.__main__ import main as wiki_main
from tools.wiki.facts import encoded, mediainfo

TOOLS = shutil.which(os.environ.get("OBC_OSMIUM", "osmium")) and shutil.which("zstd") or os.environ.get("CI")


def entity(qid, claims=None, sitelinks=None):
    return dict(type="item", id=qid, lastrevid=7, claims=claims or {}, labels={"en": {"value": qid}},
                sitelinks=sitelinks if sitelinks is not None else {"enwiki": {"site": "enwiki", "title": "Castle"}} if qid == "Q1" else {})


def claim(value):
    return dict(rank="normal", mainsnak=dict(datavalue=dict(value=value)))


def page(revision=11, name="Castle", pageid=42, language="en", qid="Q1", redirects=("Old Castle",)):
    return dict(name=name, identifier=pageid, in_language={"identifier": language}, namespace={"identifier": 0},
                main_entity={"identifier": qid} if qid else None, version={"identifier": revision},
                redirects=[{"name": alias} for alias in redirects], date_modified="2026-01-01T00:00:00Z",
                license=[{"url": "https://creativecommons.org/licenses/by-sa/4.0/"}],
                article_body={"html": f'<html about="https://{language}.wikipedia.org/wiki/Special:Redirect/revision/{revision}"><head><meta property="mw:pageId" content="{pageid}"/></head><body><section data-mw-section-id="0"><p>{name} is a fortified building beside a river.</p></section><section data-mw-section-id="1"><p>Discard this history.</p></section></body></html>'})


def archive(path, values):
    data = b"".join(encoded(value) for value in values)
    with tarfile.open(path, "w:gz") as output:
        member = tarfile.TarInfo("project_0.ndjson")
        member.size = len(data)
        output.addfile(member, BytesIO(data))


def sql(table, columns, values):
    declared = b"".join(b"  `%s` int(10) NOT NULL,\n" % column.encode() for column in columns.split())
    return b"CREATE TABLE `%s` (\n%s  PRIMARY KEY (`id`)\n);\nINSERT INTO `%s` VALUES %s;\n" % (table.encode(), declared, table.encode(), values)


def lines(path, values):
    path.write_text("".join(json.dumps(value) + "\n" for value in values))
    return path


class Wiki(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        self.work = self.root / "work"
        self.db = catalog.connect(self.work)
        self.addCleanup(self.db.close)

    def dump(self, values=None):
        """Q1 is a castle (P31 Q2, P279 Q3) in Q4, in Q5. Q999 is unrelated."""
        path = self.root / "wikidata.json.bz2"
        values = values or [entity("Q1", {"P31": [claim({"id": "Q2"})], "P131": [claim({"id": "Q4"})], "P18": [claim("Hill.jpg")]}),
                            entity("Q4", {"P131": [claim({"id": "Q5"})]}), entity("Q5"), entity("Q3"),
                            entity("Q2", {"P279": [claim({"id": "Q3"})]}), entity("Q999")]
        path.write_bytes(bz2.compress(b"[\n" + b",\n".join(encoded(value).strip() for value in values) + b"\n]\n"))
        return path

    def wikidata(self, path=None, sql=()):
        for name, data in sql:
            source = self.root / f"{name}.sql.gz"
            source.write_bytes(gzip.compress(data))
            catalog.sql(self.db, self.work, name, source, dumps.sha(source))
        path = path or self.dump()
        catalog.wikidata(self.db, self.work, path, dumps.sha(path))

    def articles(self, selected, sources, langlinks=()):
        catalog.want(self.db, self.work, selected)
        for language, data in langlinks:
            source = self.root / f"{language}-langlinks.sql.gz"
            source.write_bytes(gzip.compress(data))
            catalog.sql(self.db, self.work, "langlinks:" + language, source, dumps.sha(source))
        for language in ("es", "fr", "de", "en"):
            if language in sources:
                catalog.articles(self.db, self.work, language, sources[language], dumps.sha(sources[language]))
        catalog.links(self.db)

    def subjects(self, *values):
        subjects.load(self.db, self.work, lines(self.root / "subjects.ndjson", values))

    def test_two_passes_keep_subjects_and_their_closure_from_compressed_dumps(self):
        self.subjects(dict(osm="n1", wikidata="Q1", wikipedia="en:Old_Castle"))
        with self.assertRaisesRegex(ValueError, "import step first"):
            catalog.want(self.db, self.work, ["Q1"])
        self.wikidata()
        self.assertEqual({key for kind, key in self.db.execute("SELECT kind, key FROM facts") if kind == "entity"},
                         {"Q1", "Q2", "Q3", "Q4", "Q5"})
        self.assertEqual(catalog.fact(self.db, "link", "wikidata:Q1")["identity"], "Q1")
        wiki = self.root / "en.tar.gz"
        archive(wiki, [page(), page(12)])
        self.articles(["Q1"], {"en": wiki})
        article = catalog.fact(self.db, "article", "en:Castle")
        self.assertEqual((article["revision"], article["identity"]), (12, "en:42"))
        self.assertNotIn("history", article["lead_html"])
        link = catalog.fact(self.db, "link", "wikipedia:en:Old_Castle")
        self.assertEqual(link["identity"], "Q1")
        self.assertEqual(link["proof"]["aliases"], [{"from": "Old Castle", "to": "Castle"}])
        self.assertIsNone(catalog.fact(self.db, "article", "en:Old Castle"))

    def test_unselected_article_languages_are_explicit_omissions_and_not_photo_inputs(self):
        self.subjects(dict(osm="n1", wikidata="Q1"))
        self.wikidata(self.dump([entity("Q1", sitelinks={
            "enwiki": {"site": "enwiki", "title": "Castle"},
            "dewiki": {"site": "dewiki", "title": "Burg"},
        })]))
        catalog.want(self.db, self.work, ["Q1"], ["en"])
        self.assertEqual(self.db.execute("SELECT language, title FROM wanted").fetchall(), [("en", "Castle")])
        self.assertEqual(catalog.fact(self.db, "article", "de:Burg"), dict(
            kind="article", key="de:Burg", status="missing", reason="language_not_prepared"))
        wiki = self.root / "en.tar.gz"
        archive(wiki, [page()])
        catalog.articles(self.db, self.work, "en", wiki, dumps.sha(wiki))
        self.assertEqual(catalog.fact(self.db, "article", "en:Castle")["status"], "present")
        with self.assertRaisesRegex(ValueError, "changed"):
            catalog.want(self.db, self.work, ["Q1"], ["en", "de"])

    def test_store_objects_without_a_suffix_are_read_by_their_first_bytes(self):
        self.subjects(dict(osm="n1", wikidata="Q1"))
        objects = self.root / "objects"
        objects.mkdir()
        dump, wiki = objects / "ab", objects / "cd"
        self.dump().rename(dump)
        archive(wiki, [page()])
        self.wikidata(dump)
        self.articles(["Q1"], {"en": wiki})
        self.assertEqual(catalog.fact(self.db, "entity", "Q1")["status"], "present")
        self.assertEqual(catalog.fact(self.db, "article", "en:Castle")["status"], "present")
        unknown = objects / "ef"
        unknown.write_text("[\n" + json.dumps(entity("Q1")) + "\n]\n")
        with self.assertRaisesRegex(ValueError, "not a bz2, gzip or tar file"):
            next(dumps.lines(unknown))

    def test_sql_redirects_item_sitelinks_and_language_links_resolve_without_api(self):
        self.subjects(dict(osm="n1", wikidata="Q90"), dict(osm="n2", wikipedia="ja:CastleJA"), dict(osm="n3", wikidata="Q999"),
                      dict(osm="n4", wikipedia="de:Burg"), dict(osm="n5", wikipedia="en:Unlinked"))
        self.wikidata(self.dump([entity("Q1", sitelinks={"jawiki": {"site": "jawiki", "title": "CastleJA"}})]), sql=[
            ("wikidata_pages", sql("page", "page_id page_namespace page_title page_restrictions page_is_redirect", b"(1,0,'Q90','',1),(2,0,'Q1','',0)")),
            ("wikidata_redirects", sql("redirect", "rd_from rd_namespace rd_title rd_interwiki rd_fragment", b"(1,0,'Q1','','')"))])
        self.assertEqual(catalog.fact(self.db, "entity", "Q90")["identity"], "Q1")
        self.assertEqual(catalog.fact(self.db, "link", "wikidata:Q90")["identity"], "Q1")
        self.assertEqual(catalog.fact(self.db, "link", "wikipedia:ja:CastleJA")["identity"], "Q1")
        self.assertEqual(catalog.fact(self.db, "link", "wikidata:Q999")["status"], "missing")

    def test_a_title_cut_inside_a_character_does_not_stop_the_import(self):
        data = sql("langlinks", "ll_from ll_lang ll_title", b"(1,'bn','\xe0\xa6\xb8\xe0'),\n(1,'de','Burg')")
        self.assertEqual(list(dumps.rows(BytesIO(data), "langlinks", ("ll_lang", "ll_title"))),
                         [["bn", "\u09b8\ufffd"], ["de", "Burg"]])

    def test_a_redirect_to_an_item_missing_from_the_dump_is_missing(self):
        self.subjects(dict(osm="n1", wikidata="Q90"))
        self.db.execute("INSERT INTO redirects VALUES('Q90','Q1')")
        self.wikidata(self.dump([entity("Q5")]))
        self.assertEqual((catalog.fact(self.db, "entity", "Q90")["status"], catalog.fact(self.db, "link", "wikidata:Q90")["status"]),
                         ("missing", "missing"))
        self.assertTrue(catalog.complete(self.db, "wikidata:2"))

    def test_a_cyclic_redirect_chain_fails_its_subject_and_not_the_import(self):
        self.subjects(dict(osm="n1", wikidata="Q7"), dict(osm="n2", wikidata="Q1"))
        self.db.executemany("INSERT INTO redirects VALUES(?,?)", [("Q7", "Q8"), ("Q8", "Q7")])
        self.wikidata()
        self.assertTrue(catalog.complete(self.db, "wikidata:2"))
        self.assertEqual(catalog.fact(self.db, "link", "wikidata:Q1")["identity"], "Q1")
        self.assertIsNone(catalog.fact(self.db, "entity", "Q7"))
        self.assertEqual({row[0] for row in self.db.execute("SELECT key FROM failures")}, {"Q7", "wikidata:Q7"})

    def test_a_page_without_an_item_links_titles_that_have_articles(self):
        self.subjects(dict(osm="n1", wikipedia="de:Burg"), dict(osm="n2", wikipedia="en:Lonely"))
        self.wikidata()
        en, de = self.root / "en.tar.gz", self.root / "de.tar.gz"
        archive(en, [page(name="Unlinked", qid=None, redirects=["Castle"]), page(name="Lonely", pageid=44, qid=None, redirects=[])])
        archive(de, [page(name="Burg", pageid=43, language="de", qid=None, redirects=[])])
        self.articles([], {"en": en, "de": de}, langlinks=[("de", sql("langlinks", "ll_from ll_lang ll_title", b"(43,'en','Castle'),(43,'fr','Chateau \\'A\\'')"))])
        link = catalog.fact(self.db, "link", "wikipedia:de:Burg")
        self.assertEqual(link["identity"], "wiki-en-42")
        self.assertEqual(link["sitelinks"], {"dewiki": {"title": "Burg"}, "enwiki": {"title": "Castle"}})
        self.assertEqual(link["proof"]["canonical"]["aliases"], [{"from": "Castle", "to": "Unlinked"}])
        self.assertEqual(link["proof"]["langlinks"]["fr"], "Chateau 'A'")
        for site, value in link["sitelinks"].items():
            self.assertEqual(catalog.fact(self.db, "article", f"{site.removesuffix('wiki')}:{value['title']}")["status"], "present")
        self.assertIsNone(catalog.fact(self.db, "link", "wikipedia:en:Lonely"))
        self.assertIn("langlinks SQL", self.db.execute("SELECT reason FROM failures WHERE key='wikipedia:en:Lonely'").fetchone()[0])

    def test_rendered_revision_owns_the_pin_and_a_wrong_page_proof_keeps_no_fact(self):
        declared = page()
        declared["version"]["identifier"] = 99
        article = catalog.article(declared, "en")
        self.assertEqual((article["revision"], article["representation"]["declared_revision"]), (11, 99))
        self.assertIn("oldid=11", article["url"])
        self.assertIsNone(article["timestamp"])
        self.subjects(dict(osm="n1", wikidata="Q1", wikipedia="en:Old_Castle"))
        self.wikidata()
        wrong = page()
        wrong["article_body"]["html"] = wrong["article_body"]["html"].replace('content="42"', 'content="43"')
        archive(self.root / "en.tar.gz", [wrong])
        self.articles(["Q1"], {"en": self.root / "en.tar.gz"})
        self.assertIsNone(catalog.fact(self.db, "article", "en:Castle"))
        self.assertIsNone(catalog.fact(self.db, "link", "wikipedia:en:Old_Castle"))
        reasons = dict(self.db.execute("SELECT key, reason FROM failures"))
        self.assertIn("revision", reasons["en:Castle"])
        self.assertEqual(reasons["wikipedia:en:Old_Castle"], reasons["en:Old Castle"])

    def test_an_interrupted_import_starts_again_and_a_changed_input_never_resumes(self):
        self.subjects(dict(osm="n1", wikidata="Q1"))
        self.wikidata()
        catalog.want(self.db, self.work, ["Q1"])
        wiki = self.root / "en.tar.gz"
        archive(wiki, [page()])
        with patch.object(catalog, "article", side_effect=RuntimeError("stopped")), self.assertRaises(RuntimeError):
            catalog.articles(self.db, self.work, "en", wiki, dumps.sha(wiki))
        self.assertIsNone(catalog.complete(self.db, "wikipedia:en"))
        catalog.articles(self.db, self.work, "en", wiki, dumps.sha(wiki))
        self.assertTrue(catalog.complete(self.db, "wikipedia:en"))
        self.assertEqual(catalog.fact(self.db, "article", "en:Castle")["status"], "present")
        changed = self.dump([entity("Q1")])
        with self.assertRaisesRegex(ValueError, "input changed"):
            catalog.wikidata(self.db, self.work, changed, dumps.sha(changed))

    def test_photos_are_fetched_only_for_the_selected_subjects(self):
        self.subjects(dict(osm="n1", wikidata="Q1", box=[7.1, 47.1, 7.1, 47.1]),
                      dict(osm="n2", wikidata="Q2", box=[7.2, 47.2, 7.2, 47.2]))
        self.wikidata(self.dump([entity("Q1", {"P18": [claim("Hill.jpg")]}), entity("Q2", {"P18": [claim("Other.jpg")]})]))
        thumb = "https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Hill.jpg/500px-Hill.jpg"
        info = dict(url="https://upload.wikimedia.org/wikipedia/commons/a/ab/Hill.jpg", thumburl=thumb, thumbwidth=500,
                    timestamp="2026-01-01T00:00:00Z", sha1="abc", extmetadata={"Artist": {"value": "A"}})
        asked, description = [], [20]

        def respond(request, timeout):
            url = request.full_url
            asked.append(url)
            query = {key: value[0] for key, value in parse_qs(urlparse(url).query).items()}
            if url == thumb:
                data = b"jpeg bytes"
            elif query.get("list") == "categorymembers":
                data = json.dumps({"query": {"categorymembers": [{"title": "File:Hill.jpg", "ns": 6}]}}).encode()
            elif query.get("action") == "wbgetentities":
                data = json.dumps({"entities": {"M7": {"id": "M7", "lastrevid": 30, "statements": {}}}}).encode()
            else:
                self.assertEqual(query["titles"], "File:Hill.jpg")
                data = json.dumps({"query": {"pages": [dict(pageid=7, title="File:Hill.jpg", revisions=[{"revid": description[0]}], imageinfo=[info])]}}).encode()
            response = Mock(url=url, status=200, headers={"Content-Length": str(len(data))})
            response.read.side_effect = [data, b""]
            response.__enter__ = Mock(return_value=response)
            response.__exit__ = Mock(return_value=False)
            return response

        request = {"Q1": {"categories": ["Hill"], "commons": ["Hill.jpg"]}}
        with patch("tools.landmark_capture.urlopen", side_effect=respond), patch("tools.landmark_capture.time.sleep"):
            commons.acquire(self.db, self.work, request)
            # A description edit between the rounds: the agreeing fresh checks keep the photo.
            description[0] = 21
            request["Q1"]["files"] = ["Hill.jpg"]
            commons.acquire(self.db, self.work, request)
            answered = len(asked)
            commons.acquire(self.db, self.work, request)
        self.assertEqual(len(asked), answered)
        self.assertFalse(any("Other" in url for url in asked))
        self.assertEqual(asked.count(thumb), 1)
        self.assertEqual(catalog.fact(self.db, "commons", "File:Hill.jpg")["revision"], 21)
        photo = catalog.fact(self.db, "file", "File:Hill.jpg")
        self.assertEqual(photo["asset"]["input"], "thumbnail500")
        self.assertEqual(photo["revision_after"]["query"]["pages"]["compact"]["imageinfo"][0]["description_revision"], 21)
        self.assertEqual((self.work / photo["asset"]["path"]).read_bytes(), b"jpeg bytes")
        self.assertEqual(catalog.fact(self.db, "mediainfo", "M7")["revision"], 30)
        if TOOLS:
            shards.write(self.db, self.work, self.root / "out")
            q1, q2 = (members(self.root / "out" / f"cells/{subjects.cells([lon, lat, lon, lat])[0]}.tar.zst") for lon, lat in ((7.1, 47.1), (7.2, 47.2)))
            self.assertIn(f"content/assets/{photo['asset']['sha256']}", q1)
            self.assertFalse(any(name.startswith("content/assets/") for name in q2))

    def test_candidates_are_the_classes_the_subjects_and_the_resolved_links_of_peaks(self):
        self.subjects(dict(osm="n1", wikidata="Q1", peak=True, box=[7.1, 47.1, 7.1, 47.1]),
                      dict(osm="w2", wikidata="Q999", box=[7.2, 47.2, 7.2, 47.2]))
        self.wikidata()
        out = self.root / "candidates.jsonl"
        catalog.candidates(self.db, out)
        lines = [json.loads(line) for line in out.read_text().splitlines()]
        self.assertEqual([line["class"]["key"] for line in lines if "class" in line], ["Q2", "Q3"])
        self.assertEqual([line["subject"]["key"] for line in lines if "subject" in line], ["Q1", "Q999"])
        self.assertEqual([(line["peak"], line["links"]) for line in lines if "peak" in line], [("Q1", ["wikidata:Q1"])])

    def test_the_facts_of_a_chunk_hold_its_subjects_and_the_classes_of_their_types(self):
        self.subjects(dict(osm="n1", wikidata="Q1", box=[7.1, 47.1, 7.1, 47.1]), dict(osm="n2", wikidata="Q999"))
        self.wikidata()
        shards.export(self.db, self.root / "facts", ["Q1"], [])
        found = facts({str(path.relative_to(self.root / "facts")): path.read_bytes()
                       for path in (self.root / "facts").rglob("*.json")})
        self.assertEqual(found, {("link", "wikidata:Q1"), ("entity", "Q1"), ("entity", "Q2"), ("entity", "Q3"),
                                 ("entity", "Q4"), ("entity", "Q5")})

    def test_a_file_without_statements_depicts_nothing(self):
        raw = {"entities": {"M1": {"id": "M1", "lastrevid": 5, "type": "mediainfo", "statements": []}}}
        ((key, value),) = mediainfo(lambda url: raw, ["M1"])
        self.assertEqual((key, value["status"], value["statements"]), ("M1", "present", {"P180": []}))

    def test_a_busy_commons_leaves_its_mark_for_the_producer(self):
        requests = self.root / "requests.json"
        requests.write_text("{}")
        argv = ["tools.wiki", "commons", "--work", str(self.work), "--requests", str(requests)]
        with patch("sys.argv", argv), patch.object(commons, "acquire", side_effect=RuntimeError("Wikimedia is busy")):
            with self.assertRaises(RuntimeError):
                wiki_main()
        self.assertTrue((self.work / "busy").exists())

    @unittest.skipUnless(TOOLS, "Osmium and zstd are unavailable")
    def test_a_published_shard_is_not_written_again_and_the_text_can_stay_out(self):
        self.subjects(dict(osm="n1", wikidata="Q1", box=[7.1, 47.1, 7.1, 47.1]))
        self.wikidata()
        archive(self.root / "en.tar.gz", [page()])
        self.articles(["Q1"], {"en": self.root / "en.tar.gz"})
        out, plain = self.root / "out", self.root / "plain"
        shards.write(self.db, self.work, out)
        shards.write(self.db, self.work, plain, text=False)
        cell = json.loads((out / "index.json").read_text())["shards"][1]
        self.assertIn(("article", "en:Castle"), facts(members(out / cell["path"])))
        articles = [json.loads(data) for name, data in members(plain / cell["path"]).items() if name.startswith("content/article/")]
        self.assertEqual(articles, [{"kind": "article", "key": "en:Castle", "status": "missing"}], "no text, and readers see no article")
        (out / cell["path"]).unlink()
        shards.write(self.db, self.work, out, {cell["path"]: {"sha256": cell["sha256"], "bytes": cell["bytes"]}})
        self.assertFalse((out / cell["path"]).exists())
        self.assertEqual(json.loads((out / "index.json").read_text())["shards"][1], cell)

    @unittest.skipUnless(TOOLS, "Osmium and zstd are unavailable")
    def test_a_subject_lands_in_each_cell_it_touches_and_a_large_object_in_common(self):
        source = self.root / "planet.osm"
        source.write_text('<osm version="0.6">'
                          '<node id="1" version="1" lat="47.1" lon="7.1"><tag k="natural" v="peak"/><tag k="wikidata" v="Q1"/></node>'
                          '<node id="2" version="1" lat="47.2" lon="7.2"/><node id="3" version="1" lat="52.5" lon="13.5"/>'
                          '<node id="4" version="1" lat="47.15" lon="7.1"/><node id="5" version="1" lat="47.25" lon="7.1"/>'
                          '<way id="10" version="1"><nd ref="2"/><nd ref="3"/><tag k="waterway" v="river"/><tag k="wikidata" v="Q4"/></way>'
                          '<way id="11" version="1"><nd ref="4"/><nd ref="5"/><tag k="bridge" v="yes"/><tag k="wikidata" v="Q999"/></way>'
                          '<relation id="20" version="1"><member type="node" ref="2" role=""/><tag k="type" v="route"/><tag k="wikidata" v="Q5"/></relation>'
                          '</osm>')
        listed = self.root / "subjects.ndjson"
        subjects.extract(source, listed, self.root / "osm")
        values = [json.loads(line) for line in listed.read_text().splitlines()]
        self.assertEqual([(value["osm"], value.get("peak"), "box" in value) for value in values],
                         [("n1", True, True), ("w10", None, True), ("w11", None, True), ("r20", None, False)])
        boxed = self.root / "boxed.ndjson"
        subjects.extract(source, boxed, self.root / "osm", [7.0, 47.0, 7.12, 47.12])
        self.assertEqual([json.loads(line)["osm"] for line in boxed.read_text().splitlines()], ["n1"])
        subjects.load(self.db, self.work, listed)
        with self.assertRaisesRegex(ValueError, "import step first"):
            shards.write(self.db, self.work, self.root / "out")
        self.wikidata()
        shards.write(self.db, self.work, self.root / "out", counts={"photo_compile_failures": 2})
        index = json.loads((self.root / "out/index.json").read_text())
        # The way w11 crosses the border at 47.185920° between these two cells.
        self.assertEqual([shard["path"] for shard in index["shards"]], ["common.tar.zst", "cells/0300-0262.tar.zst", "cells/0301-0262.tar.zst"])
        common, south, north = (facts(members(self.root / "out" / shard["path"])) for shard in index["shards"])
        bridge = {("link", "wikidata:Q999"), ("entity", "Q999")}
        self.assertEqual(south, {("link", "wikidata:Q1"), ("entity", "Q1"), ("entity", "Q4"), ("entity", "Q5")} | bridge)
        self.assertEqual(north, bridge)
        # The long river, the route relation and every class.
        self.assertEqual(common, {("link", "wikidata:Q4"), ("link", "wikidata:Q5"), ("entity", "Q2"), ("entity", "Q3"),
                                  ("entity", "Q4"), ("entity", "Q5")})
        self.assertEqual([index["counts"][name] for name in ("subjects", "subjects_in_common", "photo_compile_failures")], [4, 2, 2])


def members(path):
    data = subprocess.run(["zstd", "-dc", str(path)], capture_output=True, check=True).stdout
    with tarfile.open(fileobj=BytesIO(data)) as tar:
        return {member.name: tar.extractfile(member).read() for member in tar}


def facts(contents):
    """The (kind, key) of each fact, after checking that its name is the SHA-256 of its bytes."""
    found = set()
    for name, data in contents.items():
        if name.endswith(".json"):
            assert Path(name).stem == dumps.hashlib.sha256(data).hexdigest(), name
            value = json.loads(data)
            found.add((value["kind"], value["key"]))
    return found


if __name__ == "__main__":
    unittest.main()
