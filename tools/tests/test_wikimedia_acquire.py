from pathlib import Path
from io import BytesIO
import tempfile
import unittest
from unittest.mock import Mock, patch

from tools.wikimedia_acquire import Acquisition, encoded, lead_html, original_notices
from tools.landmark_capture import MEDIA_BYTES_PER_SECOND, digest, read_source


def entity(qid, revision=1, claims=None):
    return dict(id=qid, lastrevid=revision, labels={"en": {"value": qid}}, claims=claims or {})


def claim(qid):
    return {"mainsnak": {"datavalue": {"value": {"id": qid}}}}


def page(title="Hill", revision=10, qid="Q1"):
    return dict(pageid=7, title=title, pageprops={"wikibase_item": qid} if qid else {},
                revisions=[dict(revid=revision, timestamp="2026-01-01T00:00:00Z")])


def rendered(revision=10):
    return dict(id=7, title="Hill", latest=dict(id=revision, timestamp="2026-01-01T00:00:00Z"),
                license={"url": "https://creativecommons.org/licenses/by-sa/4.0/"},
                html='<html><body><section data-mw-section-id="0"><p>Hill &amp; mountain.</p></section><section data-mw-section-id="1"><p>History.</p></section></body></html>')


class AcquisitionTests(unittest.TestCase):
    def operation(self, root, inputs=(), responses=(), check_id="check1"):
        transport = Mock()
        transport.json.side_effect = responses
        return Acquisition(root / "work", root / "out", check_id, inputs, transport=transport)

    def inputs(self, operation):
        manifest = operation.finish()
        return [dict(record, path=str(operation.out / record["path"])) for record in manifest["records"]]

    def test_overlapping_requests_reuse_pinned_identity_offline(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = self.operation(root / "first", responses=[{"entities": {"Q1": entity("Q1"), "Q2": entity("Q2")}}])
            first.entities(["Q1", "Q2"])
            inputs = self.inputs(first)
            second = self.operation(root / "second", inputs=inputs)
            second.entities(["Q2"])
            manifest = second.finish()
            self.assertTrue(manifest["complete"])
            self.assertEqual([record["key"] for record in manifest["records"]], ["Q2"])
            self.assertEqual(manifest["records"][0]["sha256"], inputs[1]["sha256"])
            second.transport.json.assert_not_called()

    def test_newer_revision_inputs_replace_old_journal_but_unchanged_successes_resume(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original = self.operation(root / "operation", responses=[{"entities": {"Q1": entity("Q1", 7), "Q2": entity("Q2", 7)}}])
            original.entities(["Q1", "Q2"])
            original_inputs = self.inputs(original)
            newer = self.operation(root / "refresh", responses=[{"entities": {"Q1": entity("Q1", 8)}}])
            newer.entities(["Q1"])
            incoming = self.inputs(newer)
            incoming.append(original_inputs[1])
            for pin in incoming: pin["checked_at"] = "2099-01-01T00:00:00Z"
            resumed = self.operation(root / "operation", inputs=incoming)
            resumed.entities(["Q1", "Q2"])
            result = resumed.finish()
            self.assertEqual({pin["key"]: pin["revision"] for pin in result["records"]}, {"Q1": 8, "Q2": 7})
            self.assertTrue(all(pin["checked_at"] == "2099-01-01T00:00:00Z" for pin in result["records"]))
            resumed.transport.json.assert_not_called()

    def test_refresh_checks_revision_and_only_fetches_changed_facts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = self.operation(root / "first", responses=[{"entities": {"Q1": entity("Q1"), "Q2": entity("Q2")}}])
            first.entities(["Q1", "Q2"])
            inputs = self.inputs(first)
            changed = self.operation(root / "next", inputs=inputs, responses=[
                {"entities": {"Q1": dict(id="Q1", lastrevid=1), "Q2": dict(id="Q2", lastrevid=2)}},
                {"entities": {"Q2": entity("Q2", 2)}}])
            changed.entities(["Q1", "Q2"], refresh=True)
            manifest = changed.finish()
            self.assertEqual([record["revision"] for record in manifest["records"]], [1, 2])
            self.assertEqual(manifest["records"][0]["sha256"], inputs[0]["sha256"])
            self.assertIn("props=info", changed.transport.json.call_args_list[0].args[1])
            self.assertIn("ids=Q2&", changed.transport.json.call_args_list[1].args[1])

    def test_missing_is_refreshable_and_failure_is_unresolved(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = self.operation(root / "first", responses=[{"entities": {"Q1": {"id": "Q1", "missing": ""}}}])
            first.entities(["Q1"])
            inputs = self.inputs(first)
            self.assertEqual(inputs[0]["status"], "missing")
            self.assertNotIn("revision", inputs[0])
            failed = self.operation(root / "failed", inputs=inputs, responses=[None])
            failed.entities(["Q1"], refresh=True)
            manifest = failed.finish()
            self.assertFalse(manifest["complete"])
            self.assertEqual(manifest["records"], [])
            revived = self.operation(root / "revived", inputs=inputs, responses=[{"entities": {"Q1": entity("Q1", 2)}}, {"entities": {"Q1": entity("Q1", 2)}}])
            revived.entities(["Q1"], refresh=True)
            self.assertEqual(revived.finish()["records"][0]["revision"], 2)

    def test_interruption_resumes_successes_and_refresh_does_not_reuse_old_journal(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = self.operation(root, responses=[{"entities": {"Q1": entity("Q1")}}])
            first.entities(["Q1"])
            resumed = self.operation(root, responses=[{"entities": {"Q2": entity("Q2")}}])
            resumed.entities(["Q1", "Q2"])
            self.assertTrue(resumed.finish()["complete"])
            self.assertEqual(resumed.transport.json.call_count, 1)
            refreshed = self.operation(root, check_id="check2", responses=[{"entities": {"Q1": entity("Q1", 3)}}])
            refreshed.entities(["Q1"])
            self.assertEqual(refreshed.finish()["records"][0]["revision"], 3)

    def test_corrupt_or_relabelled_input_is_rejected_before_network(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = self.operation(root / "first", responses=[{"entities": {"Q1": entity("Q1")}}])
            first.entities(["Q1"])
            inputs = self.inputs(first)
            with self.assertRaisesRegex(ValueError, "identity changed"):
                self.operation(root / "second", inputs=[dict(inputs[0], key="Q2")])
            Path(inputs[0]["path"]).write_bytes(b"corrupt")
            with self.assertRaisesRegex(ValueError, "digest changed"):
                self.operation(root / "third", inputs=inputs)

    def test_articles_batch_exact_aliases_and_pin_only_matching_current_revision(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            raw = {"query": {"normalized": [{"from": "the_Hill", "to": "The Hill"}],
                             "redirects": [{"from": "The Hill", "to": "Hill"}], "pages": [page()]}}
            operation = self.operation(root, responses=[raw, rendered()])
            operation.articles([dict(language="en", title="the_Hill", qid="Q1")])
            result = operation.value("article", "en:the_Hill")
            self.assertEqual(result["identity"], "en:7")
            self.assertEqual(result["revision"], 10)
            self.assertEqual(len(result["aliases"]), 2)
            self.assertNotIn("History", result["lead_html"])
            self.assertIn("with_html", operation.transport.json.call_args.args[1])
            self.assertTrue(operation.finish()["complete"])
            changed = self.operation(root / "changed", responses=[raw, rendered(11)])
            changed.articles([dict(language="en", title="the_Hill", qid="Q1")])
            self.assertEqual(changed.finish()["failures"][0]["reason"], "article-changed-during-acquisition")
            self.assertIsNone(changed.value("article", "en:the_Hill"))

    def test_article_refresh_reuses_html_and_checks_subject_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = self.operation(root / "first", responses=[{"query": {"pages": [page()]}}, rendered()])
            first.articles([dict(language="en", title="Hill", qid="Q1")])
            inputs = self.inputs(first)
            refresh = self.operation(root / "refresh", inputs=inputs, responses=[{"query": {"pages": [page()]}}])
            refresh.articles([dict(language="en", title="Hill", qid="Q1")], refresh=True)
            self.assertEqual(refresh.transport.json.call_count, 1)
            self.assertEqual(refresh.finish()["records"][0]["sha256"], inputs[0]["sha256"])
            bad = self.operation(root / "bad", inputs=inputs)
            bad.articles([dict(language="en", title="Hill", qid="Q2")])
            self.assertFalse(bad.finish()["complete"])

    def test_dependency_inputs_are_shared_across_subjects(self):
        with tempfile.TemporaryDirectory() as temporary:
            operation = self.operation(Path(temporary), responses=[
                {"entities": {"Q1": entity("Q1", claims={"P31": [claim("Q10")], "P17": [claim("Q20")], "P131": [claim("Q30")]}),
                              "Q2": entity("Q2", claims={"P31": [claim("Q10")], "P17": [claim("Q20")], "P131": [claim("Q30")]})}},
                {"entities": {"Q10": entity("Q10", claims={"P279": [claim("Q11")]})}},
                {"entities": {"Q11": entity("Q11")}},
                {"entities": {"Q20": entity("Q20", claims={"P37": [claim("Q188")]})}},
                {"entities": {"Q30": entity("Q30", claims={"P131": [claim("Q30")], "P37": [claim("Q188")]})}}])
            operation.dependencies(["Q1", "Q2"])
            self.assertEqual([record["key"] for record in operation.finish()["records"]], ["Q1", "Q10", "Q11", "Q2", "Q20", "Q30"])
            self.assertEqual(operation.transport.json.call_count, 5)

    def test_new_exact_alias_reuses_admitted_article_page_revision(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = self.operation(root / "first", responses=[{"query": {"pages": [page()]}}, rendered()])
            first.articles([dict(language="en", title="Hill", qid="Q1")])
            inputs = self.inputs(first)
            alias = {"query": {"normalized": [{"from": "Another_Hill", "to": "Another Hill"}],
                               "redirects": [{"from": "Another Hill", "to": "Hill"}], "pages": [page()]}}
            second = self.operation(root / "second", inputs=inputs, responses=[alias])
            second.articles([dict(language="en", title="Another_Hill", qid="Q1")])
            result = second.value("article", "en:Another_Hill")
            self.assertEqual(second.transport.json.call_count, 1)
            self.assertEqual(result["identity"], "en:7")
            self.assertEqual(result["revision"], 10)
            self.assertEqual(result["aliases"], alias["query"]["normalized"] + alias["query"]["redirects"])
            self.assertEqual(result["lead_html"], first.value("article", "en:Hill")["lead_html"])
            self.assertTrue(second.finish()["complete"])

    def test_two_exact_aliases_in_one_operation_acquire_one_article(self):
        with tempfile.TemporaryDirectory() as temporary:
            alias = {"query": {"normalized": [{"from": "Another_Hill", "to": "Another Hill"}],
                               "redirects": [{"from": "Another Hill", "to": "Hill"}], "pages": [page()]}}
            operation = self.operation(Path(temporary), responses=[alias, rendered()])
            operation.articles([dict(language="en", title="Hill", qid="Q1"), dict(language="en", title="Another_Hill", qid="Q1")])
            self.assertEqual(operation.transport.json.call_count, 2)
            self.assertEqual(operation.value("article", "en:Hill")["aliases"], [])
            self.assertEqual(len(operation.value("article", "en:Another_Hill")["aliases"]), 2)
            self.assertTrue(operation.finish()["complete"])

    def test_entity_redirect_requires_exact_proof(self):
        with tempfile.TemporaryDirectory() as temporary:
            operation = self.operation(Path(temporary), responses=[{"entities": {"Q1": dict(entity("Q2"), redirects={"from": "Q1", "to": "Q2"}), "Q3": entity("Q4")}}])
            operation.entities(["Q1", "Q3"])
            self.assertEqual(operation.value("entity", "Q1")["identity"], "Q2")
            self.assertIsNone(operation.value("entity", "Q3"))
            self.assertFalse(operation.finish()["complete"])

    def test_peak_exact_wiki_link_selects_canonical_supported_language(self):
        with tempfile.TemporaryDirectory() as temporary:
            italian = dict(page("Monte", qid=None), langlinks=[{"lang": "en", "title": "Hill"}])
            english = dict(page("Hill", qid=None), langlinks=[{"lang": "it", "title": "Monte"}])
            operation = self.operation(Path(temporary), responses=[{"query": {"pages": [italian]}}, {"query": {"pages": [english]}}])
            self.assertEqual(operation.links(["wikipedia:it:Monte"]), ["wiki-en-7"])
            value = operation.value("link", "wikipedia:it:Monte")
            self.assertEqual(value["proof"]["canonical"]["language"], "en")
            self.assertEqual(value["sitelinks"], {"en": {"title": "Hill"}})

    def test_incomplete_language_links_and_pressure_are_not_missing(self):
        with tempfile.TemporaryDirectory() as temporary:
            raw = {"query": {"pages": [page(qid=None)]}, "continue": {"llcontinue": "next", "continue": "||"}}
            operation = self.operation(Path(temporary), responses=[raw, None])
            operation.links(["wikipedia:en:Hill"])
            manifest = operation.finish()
            self.assertEqual(manifest["records"], [])
            self.assertFalse(manifest["complete"])
            stopped = self.operation(Path(temporary) / "stopped", responses=[RuntimeError("Wikimedia is busy")])
            with self.assertRaisesRegex(RuntimeError, "busy"):
                stopped.entities(["Q1", "Q2"])
            self.assertEqual(stopped.transport.json.call_count, 1)

    def test_lead_compaction_preserves_notices_and_omits_other_sections(self):
        html = '<html><body><section data-mw-section-id="0" data-mw="large"><div class="source-attribution">From a free source.</div><p>Hill<br/>Two &amp; three.</p></section><section data-mw-section-id="1"><p>History.</p><div class="attribution">Required notice.</div></section></body></html>'
        lead = lead_html(html)
        self.assertIn("Two &amp; three", lead)
        self.assertNotIn("data-mw=", lead)
        self.assertNotIn("History", lead)
        self.assertEqual(original_notices(html), '<div class="source-attribution">From a free source.</div><div class="attribution">Required notice.</div>')
        self.assertEqual(lead_html('<div class="mw-parser-output"><p>Lead.</p><h2>Later</h2><p>Body.</p></div>'), '<div class="mw-parser-output"><p>Lead.</p></div>')

    def test_commons_batches_metadata_and_depicts_with_separate_revisions(self):
        with tempfile.TemporaryDirectory() as temporary:
            info = dict(url="https://upload.wikimedia.org/original.jpg", thumburl="https://upload.wikimedia.org/500px.jpg", timestamp="2026-01-01T00:00:00Z", sha1="abc", extmetadata={"Permission": {"value": "Credit required."}})
            commons_page = dict(page("File:Hill.jpg", revision=20), imageinfo=[info], categories=[{"title": "Category:Hill"}])
            raw = {"query": {"pages": [commons_page, {"title": "File:Absent.jpg", "missing": True}]}}
            operation = self.operation(Path(temporary), responses=[raw, {"entities": {"M7": {"id": "M7", "lastrevid": 30, "statements": {"P180": [claim("Q1")]}}}}])
            operation.commons(["Hill.jpg", "Absent.jpg"])
            self.assertTrue(operation.finish()["complete"])
            record = operation.value("commons", "File:Hill.jpg")
            self.assertEqual(record["revision"], 20)
            self.assertEqual(record["file_revision"]["sha1"], "abc")
            self.assertEqual(record["categories"], ["Category:Hill"])
            self.assertEqual(record["imageinfo"]["extmetadata"]["Permission"]["value"], "Credit required.")
            self.assertEqual(operation.value("mediainfo", "M7")["revision"], 30)
            self.assertIn("iiurlwidth=500", operation.transport.json.call_args_list[0].args[1])

    def test_deliberately_bounded_category_is_complete_but_unfinished_metadata_is_not(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            members = [dict(pageid=index, title=f"File:{index}.jpg", ns=6, type="file") for index in range(100)]
            operation = self.operation(root / "bounded", responses=[{"query": {"categorymembers": members}, "continue": {"cmcontinue": "next"}}])
            with patch("tools.landmark_capture.PHOTO_FALLBACK", {"files": 100}, create=True):
                operation.categories(["Hill"])
            self.assertEqual(operation.transport.json.call_count, 1)
            self.assertTrue(operation.finish()["complete"])
            category = operation.value("category", "Category:Hill")
            self.assertTrue(category["bounded"])
            self.assertTrue(category["truncated"])
            self.assertNotIn("revision", category)
            raw = {"query": {"pages": [dict(page("File:Hill.jpg"), imageinfo=[dict(timestamp="t", sha1="a")])]}, "continue": {"clcontinue": "next"}}
            incomplete = self.operation(root / "incomplete", responses=[raw, None])
            incomplete.commons(["Hill.jpg"])
            self.assertFalse(incomplete.finish()["complete"])
            self.assertIsNone(incomplete.value("commons", "File:Hill.jpg"))

    def test_media_stream_paces_bytes_below_commons_bandwidth_limit(self):
        payload = b"x" * (64 * 1024 + 10)
        with patch("tools.landmark_capture.time.monotonic", return_value=0), patch("tools.landmark_capture.time.sleep") as sleep:
            self.assertEqual(read_source(BytesIO(payload), media=True), payload)
        self.assertEqual(sleep.call_count, 2)
        self.assertEqual(sleep.call_args_list[0].args[0], 64 * 1024 / MEDIA_BYTES_PER_SECOND)
        self.assertLess(MEDIA_BYTES_PER_SECOND * 8, 25_000_000)


if __name__ == "__main__":
    unittest.main()
