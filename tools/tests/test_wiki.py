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
from tools.wiki.facts import encoded

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
        catalog.want(self.db, selected)
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
        en, de = self.root / "en.tar.gz", self.root / "de.tar.gz"
        archive(en, [page(name="Unlinked", qid=None, redirects=["Castle"])])
        archive(de, [page(name="Burg", pageid=43, language="de", qid=None, redirects=[])])
        self.articles([], {"en": en, "de": de}, langlinks=[("de", sql("langlinks", "ll_from ll_lang ll_title", b"(43,'en','Castle'),(43,'fr','Chateau \\'A\\'')"))])
        link = catalog.fact(self.db, "link", "wikipedia:de:Burg")
        self.assertEqual(link["identity"], "wiki-en-42")
        self.assertEqual(link["proof"]["canonical"]["title"], "Unlinked")
        self.assertEqual(link["proof"]["langlinks"]["fr"], "Chateau 'A'")
        self.assertEqual(set(link["sitelinks"]), {"dewiki", "enwiki"})
        self.assertTrue(catalog.fact(self.db, "article", "en:Castle"))
        self.assertIsNone(catalog.fact(self.db, "link", "wikipedia:en:Unlinked"))
        self.assertIn("langlinks SQL", self.db.execute("SELECT reason FROM failures WHERE key='wikipedia:en:Unlinked'").fetchone()[0])

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
        self.assertIn("revision", self.db.execute("SELECT reason FROM failures WHERE key='en:Castle'").fetchone()[0])

    def test_an_interrupted_import_starts_again_and_a_changed_input_never_resumes(self):
        self.subjects(dict(osm="n1", wikidata="Q1"))
        self.wikidata()
        self.articles(["Q1"], {})
        wiki = self.root / "en.tar.gz"
        archive(wiki, [page()])
        wiki.write_bytes(wiki.read_bytes()[:70])
        with self.assertRaises((EOFError, tarfile.ReadError, ValueError)):
            catalog.articles(self.db, self.work, "en", wiki, dumps.sha(wiki))
        self.assertIsNone(catalog.complete(self.db, "wikipedia:en"))
        archive(wiki, [page()])
        with self.assertRaisesRegex(ValueError, "input changed"):
            catalog.articles(self.db, self.work, "en", wiki, dumps.sha(wiki))
        changed = self.dump([entity("Q1")])
        with self.assertRaisesRegex(ValueError, "input changed"):
            catalog.wikidata(self.db, self.work, changed, dumps.sha(changed))

    def test_download_resumes_only_the_same_etag_and_never_forwards_the_token_to_storage(self):
        url = "https://api.enterprise.wikimedia.com/v2/snapshots/enwiki_namespace_0/download"
        identity = dumps.hashlib.sha256(url.encode()).hexdigest()
        (self.root / (identity + ".tar.gz")).write_bytes(b"ab")
        dumps.write_json(self.root / (identity + ".json"), {"url": url, "bytes": 4, "etag": "same"})
        head = Mock(url="https://storage.example/signed", headers={"Content-Length": "4", "ETag": "same"})
        body = Mock(status=206, headers={"Content-Range": "bytes 2-3/4"})
        body.read.side_effect = [b"cd", b""]
        for response in (head, body):
            response.__enter__ = Mock(return_value=response)
            response.__exit__ = Mock(return_value=False)
        with patch.dict(os.environ, {"OBC_WIKIMEDIA_ENTERPRISE_TOKEN": "secret"}), patch.object(dumps, "open_url", side_effect=[head, body]) as transport:
            path, digest = dumps.download({"url": url}, self.root)
        self.assertEqual(path.read_bytes(), b"abcd")
        self.assertEqual(digest, dumps.hashlib.sha256(b"abcd").hexdigest())
        request = transport.call_args_list[1].args[0]
        self.assertEqual(request.get_header("Range"), "bytes=2-")
        self.assertIsNone(request.get_header("Authorization"))
        original = dumps.Request(url, headers={"Authorization": "Bearer secret"})
        self.assertIsNone(dumps.Redirects().redirect_request(original, None, 307, "", {}, "https://storage.example/signed").get_header("Authorization"))

    def test_photos_are_fetched_only_for_the_selected_subjects(self):
        self.subjects(dict(osm="n1", wikidata="Q1", box=[7.1, 47.1, 7.1, 47.1]),
                      dict(osm="n2", wikidata="Q2", box=[7.2, 47.2, 7.2, 47.2]))
        self.wikidata(self.dump([entity("Q1", {"P18": [claim("Hill.jpg")]}), entity("Q2", {"P18": [claim("Other.jpg")]})]))
        thumb = "https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Hill.jpg/500px-Hill.jpg"
        info = dict(url="https://upload.wikimedia.org/wikipedia/commons/a/ab/Hill.jpg", thumburl=thumb, thumbwidth=500,
                    timestamp="2026-01-01T00:00:00Z", sha1="abc", extmetadata={"Artist": {"value": "A"}})
        asked = []

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
                data = json.dumps({"query": {"pages": [dict(pageid=7, title="File:Hill.jpg", revisions=[{"revid": 20}], imageinfo=[info])]}}).encode()
            response = Mock(url=url, status=200, headers={"Content-Length": str(len(data))})
            response.read.side_effect = [data, b""]
            response.__enter__ = Mock(return_value=response)
            response.__exit__ = Mock(return_value=False)
            return response

        with patch("tools.landmark_capture.urlopen", side_effect=respond), patch("tools.landmark_capture.time.sleep"):
            commons.acquire(self.db, self.work, {"Q1": {"categories": ["Hill"], "files": ["Hill.jpg"]}})
            commons.acquire(self.db, self.work, {"Q1": {"categories": ["Hill"], "files": ["Hill.jpg"]}})
        self.assertFalse(any("Other" in url for url in asked))
        self.assertEqual(asked.count(thumb), 1)
        photo = catalog.fact(self.db, "file", "File:Hill.jpg")
        self.assertEqual(photo["asset"]["input"], "thumbnail500")
        self.assertEqual(photo["revision_after"]["query"]["pages"]["compact"]["imageinfo"][0]["description_revision"], 20)
        self.assertEqual((self.work / photo["asset"]["path"]).read_bytes(), b"jpeg bytes")
        self.assertEqual(catalog.fact(self.db, "mediainfo", "M7")["revision"], 30)
        if TOOLS:
            shards.write(self.db, self.work, self.root / "out")
            q1, q2 = (members(self.root / "out" / f"cells/{subjects.cell([lon, lat, lon, lat])}.tar.zst") for lon, lat in ((7.1, 47.1), (7.2, 47.2)))
            self.assertIn(f"content/assets/{photo['asset']['sha256']}", q1)
            self.assertFalse(any(name.startswith("content/assets/") for name in q2))

    @unittest.skipUnless(TOOLS, "Osmium and zstd are unavailable")
    def test_a_subject_lands_in_its_cell_shard_and_a_large_object_in_common(self):
        source = self.root / "planet.osm"
        source.write_text('<osm version="0.6">'
                          '<node id="1" version="1" lat="47.1" lon="7.1"><tag k="natural" v="peak"/><tag k="wikidata" v="Q1"/></node>'
                          '<node id="2" version="1" lat="47.2" lon="7.2"/><node id="3" version="1" lat="49.5" lon="9.5"/>'
                          '<way id="10" version="1"><nd ref="2"/><nd ref="3"/><tag k="waterway" v="river"/><tag k="wikidata" v="Q4"/></way>'
                          '<relation id="20" version="1"><member type="node" ref="2" role=""/><tag k="type" v="route"/><tag k="wikidata" v="Q5"/></relation>'
                          '</osm>')
        listed = self.root / "subjects.ndjson"
        subjects.extract(source, listed, self.root / "osm")
        values = [json.loads(line) for line in listed.read_text().splitlines()]
        self.assertEqual([(value["osm"], value.get("peak"), "box" in value) for value in values],
                         [("n1", True, True), ("w10", None, True), ("r20", None, False)])
        subjects.load(self.db, self.work, listed)
        self.wikidata()
        shards.write(self.db, self.work, self.root / "out")
        index = json.loads((self.root / "out/index.json").read_text())
        cell = subjects.cell([7.1, 47.1, 7.1, 47.1])
        self.assertEqual([shard["path"] for shard in index["shards"]], ["common.tar.zst", f"cells/{cell}.tar.zst"])
        common, local = (facts(members(self.root / "out" / shard["path"])) for shard in index["shards"])
        # The node and its locales; the way and the route relation, and every class, in common.
        self.assertEqual(local, {("link", "wikidata:Q1"), ("entity", "Q1"), ("entity", "Q4"), ("entity", "Q5")})
        self.assertEqual(common, {("link", "wikidata:Q4"), ("link", "wikidata:Q5"), ("entity", "Q2"), ("entity", "Q3"),
                                  ("entity", "Q4"), ("entity", "Q5")})
        self.assertEqual((index["counts"]["subjects"], index["counts"]["subjects_in_common"]), (1, 2))


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
