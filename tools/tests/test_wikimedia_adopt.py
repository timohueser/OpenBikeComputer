import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from tools.landmark_capture import digest
from tools.wikimedia_adopt import RetainedCapture
from tools.wikimedia_acquire import Acquisition, encoded
from unittest.mock import Mock


class AdoptionTests(unittest.TestCase):
    def capture(self, root):
        sources = []

        def add(path, value, url):
            data = value if isinstance(value, bytes) else encoded(value)
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            sources.append(dict(path=path, url=url, sha256=digest(data), bytes=len(data), retrieved_at="2026-01-01T00:00:00Z"))

        add("entities/Q1.json", {"entities": {"Q1": {"id": "Q1", "lastrevid": 12, "labels": {}, "claims": {}, "sitelinks": {}}}}, "https://www.wikidata.org/wiki/Special:EntityData/Q1.json")
        add("classes/Q2.json", {"entities": {"Q2": {"id": "Q2", "claims": {}}}}, "https://www.wikidata.org/w/api.php?props=claims")
        add("articles/en-Q1.json", {"query": {"pages": {"7": {"pageid": 7, "title": "Hill", "pageprops": {"wikibase_item": "Q1"}, "revisions": [{"revid": 10, "timestamp": "2026-01-01T00:00:00Z"}]}}}}, "https://en.wikipedia.org/w/api.php?titles=Hill")
        html = b'<html><head><script>{"wgRevisionId":10}</script></head><body><div id="mw-content-text"><div class="mw-parser-output"><p>Hill.</p><h2>Later</h2><p>Body.</p></div></div><li id="footer-info-copyright"><a href="https://creativecommons.org/licenses/by-sa/4.0/">CC BY-SA 4.0</a></li></body></html>'
        add("articles/en-Q1.html", html, "https://en.wikipedia.org/w/index.php?title=Hill&oldid=10")
        image = b"small retained original"
        image_url = "https://upload.wikimedia.org/wikipedia/commons/0/01/Hill.jpg"
        info = dict(url=image_url, timestamp="2026-01-01T00:00:00Z", sha1=hashlib.sha1(image).hexdigest())
        add("images/image.json", {"query": {"pages": {"8": {"pageid": 8, "title": "File:Hill.jpg", "imageinfo": [info]}}}}, "https://commons.wikimedia.org/w/api.php?titles=File%3AHill.jpg")
        add("images/image.jpg", image, image_url)
        (root / "manifest.json").write_bytes(encoded(dict(sources=sources)))
        return sources

    def test_adoption_validates_entity_article_and_file_without_inventing_revisions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            capture_root = root / "capture"
            capture_root.mkdir()
            sources = self.capture(capture_root)
            capture = RetainedCapture(capture_root)
            facts = list(capture.facts())
            self.assertEqual([(value["kind"], value["revision"]) for value in facts], [("article", 10), ("entity", 12)])
            self.assertEqual(facts[0]["lead_html"], '<div class="mw-parser-output"><p>Hill.</p></div>')
            metadata = capture.unversioned_metadata()
            self.assertEqual(metadata[0]["reason"], "description-page-revision-missing")
            files = list(capture.files())
            self.assertEqual(files[0][0]["asset"]["input"], "original")
            transport = Mock()
            operation = Acquisition(root / "work", root / "out", "check", [], transport=transport)
            operation.adopt(capture_root)
            operation.entities(["Q1"])
            operation.articles([dict(language="en", title="Hill", qid="Q1")])
            self.assertTrue(operation.reuse("file", "File:Hill.jpg", False))
            manifest = operation.finish()
            self.assertTrue(manifest["complete"])
            self.assertEqual(len(manifest["assets"]), 1)
            transport.json.assert_not_called()
            for source in sources:
                self.assertEqual(digest((capture_root / source["path"]).read_bytes()), source["sha256"])
            inputs = [dict(record, path=str(operation.out / record["path"]),
                           **({"asset_path": str(operation.out / manifest["assets"][0]["path"])} if record["kind"] == "file" else {})) for record in manifest["records"]]
            warm = Acquisition(root / "warm-work", root / "warm-out", "warm", inputs, transport=Mock())
            warm.reuse("file", "File:Hill.jpg", False)
            self.assertTrue(warm.finish()["complete"])

    def test_partial_capture_reuses_only_verified_successful_link_proofs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "links").mkdir()
            (root / "outcomes").mkdir()
            value = {"entities": {"Q1": {"id": "Q2", "lastrevid": 20, "redirects": {"from": "Q1", "to": "Q2"}}}}
            data = encoded(value)
            (root / "links/wikidata-Q1.json").write_bytes(data)
            outcome = dict(path="links/wikidata-Q1.json", url="https://www.wikidata.org/w/api.php?action=wbgetentities&ids=Q1", status="ok", bytes=len(data), sha256=digest(data), retrieved_at="2026-01-01T00:00:00Z")
            (root / "outcomes/ok.json").write_bytes(encoded(outcome))
            (root / "outcomes/failed.json").write_bytes(encoded(dict(status="transport-error", path="links/failed.json")))
            facts = list(RetainedCapture(root).facts())
            self.assertEqual(len(facts), 1)
            self.assertEqual(facts[0]["identity"], "Q2")
            self.assertEqual(facts[0]["proof"]["source_sha256"], outcome["sha256"])

    def test_adopted_facts_receive_refresh_checks_and_changed_content_is_acquired(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            capture_root = root / "capture"
            capture_root.mkdir()
            self.capture(capture_root)
            for revision in (12, 13):
                with self.subTest(revision=revision):
                    transport = Mock()
                    responses = [{"entities": {"Q1": {"id": "Q1", "lastrevid": revision}}}]
                    if revision == 13:
                        responses.append({"entities": {"Q1": {"id": "Q1", "lastrevid": revision, "claims": {}, "labels": {}, "sitelinks": {}}}})
                    transport.json.side_effect = responses
                    operation = Acquisition(root / f"work-{revision}", root / f"out-{revision}", f"refresh-{revision}", [], transport=transport)
                    operation.adopt(capture_root)
                    self.assertIsNone(operation.value("entity", "Q1"))
                    original_digest = operation.inputs[("entity", "Q1")][0]["sha256"]
                    operation.entities(["Q1"], refresh=True)
                    manifest = operation.finish()
                    self.assertTrue(manifest["complete"])
                    self.assertEqual(manifest["records"][0]["revision"], revision)
                    self.assertEqual(transport.json.call_count, 1 if revision == 12 else 2)
                    if revision == 12:
                        self.assertEqual(manifest["records"][0]["sha256"], original_digest)
                    else:
                        self.assertNotEqual(manifest["records"][0]["sha256"], original_digest)

    def test_corrupt_sources_and_rendered_revision_mismatch_are_not_adopted(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sources = self.capture(root)
            html = root / "articles/en-Q1.html"
            html.write_bytes(html.read_bytes().replace(b'"wgRevisionId":10', b'"wgRevisionId":11'))
            with self.assertRaisesRegex(ValueError, "digest changed"):
                list(RetainedCapture(root).facts())
            for source in sources:
                if source["path"].endswith(".html"):
                    source.update(sha256=digest(html.read_bytes()), bytes=html.stat().st_size)
            (root / "manifest.json").write_bytes(encoded(dict(sources=sources)))
            with self.assertRaisesRegex(ValueError, "revision mismatch"):
                list(RetainedCapture(root).facts())

    def test_thumbnail_adoption_requires_coherent_before_and_after_witnesses(self):
        for outcome in ("complete", "missing-before", "missing-after", "changed-upload", "changed-credit", "changed-description"):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                sources = self.capture(root)
                metadata_path = root / "images/image.json"
                metadata = json.loads(metadata_path.read_bytes())
                info = metadata["query"]["pages"]["8"]["imageinfo"][0]
                info.update(thumburl="https://upload.wikimedia.org/500px-Hill.jpg", thumbwidth=500,
                            description_revision=20, extmetadata={"Permission": {"value": "Name the creator."}})

                def add(path, data, url):
                    target = root / path
                    target.write_bytes(data)
                    source = dict(path=path, url=url, sha256=digest(data), bytes=len(data), retrieved_at="2026-01-01T00:00:00Z")
                    sources[:] = [existing for existing in sources if existing["path"] != path]
                    sources.append(source)

                add("images/image.json", encoded(metadata), "https://commons.wikimedia.org/w/api.php?titles=File%3AHill.jpg&iiurlwidth=500")
                path = "images/image-500-1.jpg"
                add(path, b"retained thumbnail", info["thumburl"])
                for moment in ("before", "after"):
                    if outcome == f"missing-{moment}":
                        continue
                    witness = json.loads(encoded(metadata))
                    current = witness["query"]["pages"]["8"]["imageinfo"][0]
                    if moment == "after":
                        if outcome == "changed-upload":
                            current["timestamp"] = "2026-01-02T00:00:00Z"
                        elif outcome == "changed-credit":
                            current["extmetadata"]["Permission"]["value"] = "Use the required custom credit."
                        elif outcome == "changed-description":
                            current["description_revision"] = 21
                    add(f"images/image-500-1-{moment}.json", encoded(witness), "https://commons.wikimedia.org/w/api.php?titles=File%3AHill.jpg&iiurlwidth=500")
                image = dict(path=path, metadata_path="images/image.json", revision_before_path="images/image-500-1-before.json", revision_after_path="images/image-500-1-after.json")
                (root / "manifest.json").write_bytes(encoded(dict(sources=sources, places=[dict(images=[image])])))
                thumbnails = [fact for fact, _ in RetainedCapture(root).files() if fact["asset"]["input"] == "thumbnail500"]
                self.assertEqual(len(thumbnails), 1 if outcome == "complete" else 0)
                if thumbnails:
                    self.assertEqual(thumbnails[0]["revision_before"]["query"]["pages"]["compact"]["imageinfo"][0]["description_revision"], 20)
                    self.assertEqual(thumbnails[0]["revision_after"]["query"]["pages"]["compact"]["imageinfo"][0]["extmetadata"]["Permission"]["value"], "Name the creator.")


if __name__ == "__main__":
    unittest.main()
