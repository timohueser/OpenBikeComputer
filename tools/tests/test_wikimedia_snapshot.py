from io import BytesIO
import bz2
import gzip
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch

from tools import wikimedia_snapshot as snapshot
from tools.wikimedia_acquire import encoded


def entity(qid, claims=None):
    return dict(type="item", id=qid, lastrevid=7, claims=claims or {}, labels={"en": {"value": "Castle"}},
                sitelinks={"enwiki": {"title": "Castle"}} if qid == "Q1" else {})


def claim(qid):
    return dict(rank="normal", mainsnak=dict(datavalue=dict(value={"id": qid})))


def page(revision=11):
    return dict(name="Castle", identifier=42, in_language={"identifier": "en"}, namespace={"identifier": 0},
                main_entity={"identifier": "Q1"}, version={"identifier": revision}, redirects=[{"name": "Old Castle"}],
                license=[{"url": "https://creativecommons.org/licenses/by-sa/4.0/"}], date_modified="2026-01-01T00:00:00Z",
                article_body={"html": f'<html about="https://en.wikipedia.org/wiki/Special:Redirect/revision/{revision}"><head><meta property="mw:pageId" content="42"/></head><body><section data-mw-section-id="0"><p>Castle is a fortified building beside a river.</p></section><section data-mw-section-id="1"><p>Discard this history.</p></section></body></html>'})


def archive(path, values):
    data = b"".join(encoded(value) for value in values)
    with tarfile.open(path, "w:gz") as output:
        member = tarfile.TarInfo("project.ndjson")
        member.size = len(data)
        output.addfile(member, BytesIO(data))


class SnapshotTests(unittest.TestCase):
    def test_bulk_redirect_and_no_item_language_proofs_need_no_api(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = self.inputs(root)
            pages, redirects, languages = root / "page.sql.gz", root / "redirect.sql.gz", root / "de-langlinks.sql.gz"
            pages.write_bytes(gzip.compress(b"INSERT INTO `page` VALUES\n(1,0,'Q90',0),\n(2,0,'Q1',0);\n"))
            redirects.write_bytes(gzip.compress(b"INSERT INTO `redirect` VALUES (1,0,'Q1','','');\n"))
            languages.write_bytes(gzip.compress(b"INSERT INTO `langlinks` VALUES (43,'en','Castle'),(43,'fr','Chateau \\'A\\'');\n"))
            en = page()
            en["main_entity"] = None
            en["name"] = "Unlinked"
            en["redirects"] = [{"name": "Castle"}]
            # Keep this page separate from the Wikidata sitelink in the input.
            wd = Path(config["wikidata"]["path"])
            wd.write_bytes(bz2.compress(b"[\n" + encoded(dict(entity("Q1"), sitelinks={"jawiki":{"title":"CastleJA"}})) + b"]\n"))
            archive(Path(config["wikipedia"]["en"]["path"]), [en])
            de = page()
            de.update(name="Burg", identifier=43, main_entity=None, in_language={"identifier":"de"}, redirects=[])
            de["article_body"]["html"] = de["article_body"]["html"].replace("en.wikipedia", "de.wikipedia").replace('content="42"', 'content="43"')
            archive(root / "de.tar.gz", [de])
            config["wikipedia"]["de"] = {"path":str(root / "de.tar.gz"),"date":"2026-01-01"}
            config.update(wikidata_pages={"path":str(pages),"date":"2026-01-01"}, wikidata_redirects={"path":str(redirects),"date":"2026-01-01"}, langlinks={"de":{"path":str(languages),"date":"2026-01-01"}})
            snapshot.import_sources(config, root / "work")
            operation = snapshot.SnapshotAcquisition(root / "query", root / "out", "bulk", [], catalog=root / "work/catalog.sqlite")
            operation.entities(["Q90"])
            self.assertEqual(operation.value("entity", "Q90")["identity"], "Q1")
            operation.links(["wikipedia:ja:CastleJA", "wikidata:Q90", "wikidata:Q999"])
            self.assertEqual(operation.value("link", "wikidata:Q90")["identity"], "Q1")
            self.assertEqual(operation.value("link", "wikipedia:ja:CastleJA")["identity"], "Q1")
            self.assertEqual(operation.value("link", "wikidata:Q999")["status"], "missing")
            operation.links(["wikipedia:de:Burg"])
            link = operation.value("link", "wikipedia:de:Burg")
            self.assertEqual(link["identity"], "wiki-en-42")
            self.assertEqual(link["proof"]["canonical"]["title"], "Unlinked")
            self.assertEqual(link["proof"]["langlinks"]["fr"], "Chateau 'A'")
            self.assertTrue(operation.finish()["complete"])
            operation.links(["wikipedia:en:Unlinked"])
            self.assertFalse(operation.finish()["complete"])
            self.assertIn("langlinks SQL", operation.failures[-1]["reason"])

    def inputs(self, root):
        wd = root / "wikidata.json.bz2"
        wd.write_bytes(bz2.compress(b"[\n" + b",\n".join(encoded(value).strip() for value in [
            entity("Q1", {"P31": [claim("Q2")], "P131": [claim("Q4")]}),
            entity("Q4", {"P131": [claim("Q5")]}), entity("Q5"),
            entity("Q3"), entity("Q2", {"P279": [claim("Q3")]}), entity("Q999")]) + b"\n]\n"))
        wiki = root / "en.tar.gz"
        archive(wiki, [page(), page(12)])
        return dict(wikidata={"path": str(wd), "date": "2026-01-01"},
                    wikipedia={"en": {"path": str(wiki), "date": "2026-01-01"}}, entities=["Q1"])

    def test_stream_import_dependency_closure_aliases_and_offline_bundle_restore(self):
        with tempfile.TemporaryDirectory() as temporary, patch("tools.landmark_capture.urlopen", side_effect=AssertionError("network forbidden")):
            root = Path(temporary)
            config = self.inputs(root)
            work, out = root / "work", root / "published"
            snapshot.import_sources(config, work)
            operation = snapshot.SnapshotAcquisition(root / "query", root / "query-out", "one", [], catalog=work / "catalog.sqlite")
            operation.dependencies(["Q1"])
            operation.articles([{"language": "en", "title": "Old_Castle"}])
            operation.links(["wikipedia:en:Old_Castle"])
            manifest = operation.finish()
            self.assertTrue(manifest["complete"])
            self.assertEqual({pin["key"] for pin in manifest["records"] if pin["kind"] == "entity"}, {"Q1", "Q2", "Q3", "Q4", "Q5"})
            article = operation.value("article", "en:Old_Castle")
            self.assertEqual(article["revision"], 12)
            self.assertEqual(article["identity"], "en:42")
            self.assertNotIn("history", article["lead_html"])
            snapshot.retain(work / "catalog.sqlite", manifest, root / "query-out")
            snapshot.export(work, out)
            published = json.loads((out / "manifest.json").read_bytes())
            self.assertFalse(any(file["name"].endswith((".bz2", ".tar.gz")) for file in published["files"]))
            fresh = root / "fresh"
            fresh.mkdir()
            request = dict(entities=["Q1"], articles=[{"language": "en", "title": "Old_Castle"}], links=["wikipedia:en:Old_Castle"])
            missing = snapshot.hydrate(out / "index.sqlite", {}, fresh / "catalog.sqlite", request)
            self.assertTrue(missing)
            files = {file["sha256"]: str(out / file["name"]) for file in published["files"]}
            self.assertEqual(snapshot.hydrate(out / "index.sqlite", files, fresh / "catalog.sqlite", request), [])
            restored = snapshot.SnapshotAcquisition(root / "restored", root / "restored-out", "two", [], catalog=fresh / "catalog.sqlite")
            restored.dependencies(["Q1"])
            restored.articles(request["articles"])
            restored.links(request["links"])
            again = restored.finish()
            self.assertTrue(again["complete"])
            self.assertEqual({(pin["kind"], pin["key"]): pin["sha256"] for pin in manifest["records"]},
                             {(pin["kind"], pin["key"]): pin["sha256"] for pin in again["records"]})

    def test_rendered_revision_owns_the_pin_and_wrong_page_proof_blocks_publication(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = self.inputs(root)
            bad = page()
            bad["version"]["identifier"] = 99
            facts = snapshot.article(bad, "en", "2026-01-01T00:00:00Z")
            article = next(fact for fact in facts if fact["kind"] == "article")
            self.assertEqual(article["revision"], 11)
            self.assertEqual(article["representation"]["declared_revision"], 99)
            self.assertIn("oldid=11", article["url"])
            self.assertIsNone(article["timestamp"])
            bad["article_body"]["html"] = bad["article_body"]["html"].replace('content="42"', 'content="43"')
            archive(Path(config["wikipedia"]["en"]["path"]), [bad])
            snapshot.import_sources(config, root / "work")
            operation = snapshot.SnapshotAcquisition(root / "query", root / "out", "bad", [], catalog=root / "work/catalog.sqlite")
            operation.articles([{"language": "en", "title": "Castle"}])
            manifest = operation.finish()
            self.assertFalse(manifest["complete"])
            self.assertFalse(manifest["records"])
            self.assertIn("revision", manifest["failures"][0]["reason"])
            operation.articles([{"language":"en","title":"Old_Castle"}])
            operation.links(["wikipedia:en:Old_Castle"])
            self.assertFalse(operation.finish()["complete"])
            self.assertFalse(operation.records)

    def test_corrupt_archive_does_not_mark_import_complete_and_changed_inputs_do_not_resume(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = self.inputs(root)
            wiki = Path(config["wikipedia"]["en"]["path"])
            wiki.write_bytes(wiki.read_bytes()[:70])
            with self.assertRaises((EOFError, tarfile.ReadError)):
                snapshot.import_sources(config, root / "work")
            db = snapshot.connect(root / "work/catalog.sqlite")
            self.assertEqual(db.execute("SELECT complete FROM imports WHERE id='en'").fetchone()[0], 0)
            db.close()
            archive(wiki, [page()])
            with self.assertRaisesRegex(ValueError, "input changed"):
                snapshot.import_sources(config, root / "work")

    def test_bundle_tampering_and_uncovered_identities_never_become_missing_facts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            snapshot.import_sources(self.inputs(root), root / "work")
            operation = snapshot.SnapshotAcquisition(root / "query", root / "out", "gap", [], catalog=root / "work/catalog.sqlite")
            operation.entities(["Q123456"])
            manifest = operation.finish()
            self.assertFalse(manifest["complete"])
            self.assertFalse(manifest["records"])
            operation.entities(["Q1"])
            operation.failures.clear()
            manifest = operation.finish()
            snapshot.retain(root / "work/catalog.sqlite", manifest, root / "out")
            snapshot.export(root / "work", root / "published")
            published = json.loads((root / "published/manifest.json").read_bytes())
            bundle = next(file for file in published["files"] if file["kind"] == "bundle")
            path = root / "published" / bundle["name"]
            path.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "checksum"):
                snapshot.hydrate(root / "published/index.sqlite", {bundle["sha256"]: str(path)}, root / "fresh.sqlite", {"entities": ["Q1"]})

    def test_resume_requires_same_etag_range_and_never_forwards_token_to_storage(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            url = "https://api.enterprise.wikimedia.com/v2/snapshots/enwiki_namespace_0/download"
            identity = snapshot.hashlib.sha256(url.encode()).hexdigest()
            path = root / (identity + ".tar.gz")
            path.write_bytes(b"ab")
            snapshot.write_json(root / (identity + ".json"), {"url": url, "bytes": 4, "etag": "same"})
            head = Mock(url="https://storage.example/signed", headers={"Content-Length": "4", "ETag": "same"})
            head.__enter__ = Mock(return_value=head)
            head.__exit__ = Mock(return_value=False)
            body = Mock(status=206, headers={"Content-Range": "bytes 2-3/4"})
            body.read.side_effect = [b"cd", b""]
            body.__enter__ = Mock(return_value=body)
            body.__exit__ = Mock(return_value=False)
            with patch.dict(snapshot.os.environ, {"OBC_WIKIMEDIA_ENTERPRISE_TOKEN": "secret"}), patch.object(snapshot, "open_url", side_effect=[head, body]) as transport:
                restored, _ = snapshot.download({"url": url}, root)
            self.assertEqual(restored.read_bytes(), b"abcd")
            request = transport.call_args_list[1].args[0]
            self.assertEqual(request.get_header("Range"), "bytes=2-")
            self.assertIsNone(request.get_header("Authorization"))
            original = snapshot.Request(url, headers={"Authorization": "Bearer secret"})
            redirected = snapshot.Redirects().redirect_request(original, None, 307, "", {}, "https://storage.example/signed")
            self.assertIsNone(redirected.get_header("Authorization"))


if __name__ == "__main__":
    unittest.main()
