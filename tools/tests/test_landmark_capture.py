from concurrent.futures import ThreadPoolExecutor
from io import BytesIO
from pathlib import Path
import json
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.error import HTTPError

from tools.landmark_capture import BACKOFF_ATTEMPTS, BATCH, Capture, Entities, LeadImage, batches, bbox, claim_values, class_parents, digest, entity, query, retry_after, select_candidates, semantic_sources


class Response(BytesIO):
    url = "https://example.test/source"
    status = 200
    headers = {}


class LandmarkCaptureTests(unittest.TestCase):
    def test_request_restart_verifies_bytes_and_url(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            with patch("tools.landmark_capture.urlopen", return_value=Response(b'{"ok":true}')) as request:
                first = capture.fetch("raw/entity.json", Response.url)
                self.assertEqual(capture.fetch("raw/entity.json", Response.url), first)
                request.assert_called_once()
            with self.assertRaisesRegex(ValueError, "request changed"):
                capture.fetch("raw/entity.json", "https://example.test/changed")
            (Path(directory) / "raw/entity.json").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "captured bytes changed"):
                capture.fetch("raw/entity.json", Response.url)

    def test_shared_photo_is_downloaded_once(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            with patch("tools.landmark_capture.urlopen", return_value=Response(b"photo")) as request:
                with ThreadPoolExecutor(max_workers=2) as pool:
                    results = list(pool.map(lambda _: capture.fetch("photo.jpg", Response.url), range(2)))
                self.assertEqual(results[0], results[1])
                request.assert_called_once()

    def test_failed_request_is_not_empty_or_implicitly_retried(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            error = HTTPError(Response.url, 404, "Not found", {}, None)
            with patch("tools.landmark_capture.urlopen", side_effect=error) as request:
                first = capture.fetch("query.json", Response.url)
                self.assertEqual(capture.fetch("query.json", Response.url), first)
                request.assert_called_once()
            self.assertEqual(first["status"], "http-error")
            self.assertEqual(first["http_status"], 404)
            self.assertFalse((Path(directory) / "query.json").exists())

    def test_too_many_requests_backs_every_worker_off_for_the_requested_wait(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            error = HTTPError(Response.url, 429, "Too many requests", {"Retry-After": "12"}, None)
            with patch("tools.landmark_capture.urlopen", side_effect=error) as request:
                with patch("tools.landmark_capture.time.sleep") as sleep:
                    outcome = capture.fetch("query.json", Response.url)
            self.assertEqual(request.call_count, BACKOFF_ATTEMPTS, "the back-off is bounded")
            self.assertEqual(outcome["http_status"], 429)
            # The pause is on the shared schedule, so the second worker waits for it as well.
            self.assertGreater(max(call.args[0] for call in sleep.call_args_list), 11)

    def test_replication_lag_waits_instead_of_recording_a_refusal(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            responses = [Response(b'{"error":{"code":"maxlag"}}'), Response(b'{"ok":true}')]
            with patch("tools.landmark_capture.urlopen", side_effect=responses) as request:
                with patch("tools.landmark_capture.time.sleep"):
                    self.assertEqual(capture.json("query.json", Response.url), {"ok": True})
            self.assertEqual(request.call_count, 2)

    def test_retry_after_reads_seconds_a_date_and_nothing_at_all(self):
        self.assertEqual(retry_after({"Retry-After": "7"}), 7.0)
        self.assertEqual(retry_after({}, default=3.0), 3.0)
        self.assertEqual(retry_after({"Retry-After": "not a date"}, default=3.0), 3.0)
        self.assertEqual(retry_after({"Retry-After": "Thu, 01 Jan 1970 00:00:00 GMT"}), 0.0)

    def test_maxlag_is_sent_on_every_action_api_request(self):
        from tools.landmark_capture import MAXLAG, api
        self.assertIn(f"maxlag={MAXLAG}", api("www.wikidata.org", action="wbgetentities", ids="Q1"))

    def test_entities_are_fetched_fifty_at_a_time_and_read_back_by_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            ids = [f"Q{number}" for number in range(1, 121)]
            chunks = list(batches(ids))
            self.assertEqual([len(chunk) for chunk in chunks], [BATCH, BATCH, 20])
            entities = Entities(capture)
            for chunk in chunks:
                body = json.dumps({"entities": {qid: {"id": qid} for qid in chunk}}).encode()
                with patch("tools.landmark_capture.urlopen", return_value=Response(body)) as request:
                    made = entities.fetch(chunk, "entities", "info|claims")
                    self.assertEqual(request.call_count, 1, "one request for the whole chunk")
                    url = request.call_args.args[0].full_url
                    self.assertIn("ids=" + "%7C".join(chunk), url)
                (ids, path, values) = made[0]
                self.assertEqual((len(made), ids), (1, chunk))
                self.assertEqual(sorted(values), sorted(chunk))
                for qid in chunk:
                    entities.paths[qid] = path
            # Every identity resolves from the batch it was captured in, with no further request.
            with patch("tools.landmark_capture.urlopen", side_effect=AssertionError("no request")):
                self.assertEqual(entities.get("Q1")["id"], "Q1")
                self.assertEqual(entities.get("Q120")["id"], "Q120")
            self.assertIsNone(entities.get("Q999"))

    def test_a_failed_batch_is_told_apart_from_an_entity_that_is_not_there(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            entities = Entities(capture)
            body = json.dumps({"entities": {"Q1": {"id": "Q1"}, "Q2": {"id": "Q2", "missing": ""}}}).encode()
            with patch("tools.landmark_capture.urlopen", return_value=Response(body)):
                [(_, _, values)] = entities.fetch(["Q1", "Q2"], "entities", "info")
            self.assertIn("missing", values["Q2"])
            error = HTTPError(Response.url, 404, "Not found", {}, None)
            with patch("tools.landmark_capture.urlopen", side_effect=error):
                [(_, _, failed)] = entities.fetch(["Q3"], "entities", "info")
            self.assertIsNone(failed, "a failed request is not an absent entity")

    def test_a_response_the_compiler_cannot_read_is_asked_for_again_in_halves(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            entities = Entities(capture)
            chunk = ["Q38", "Q39", "Q40", "Q142"]
            def answer(request, **_kwargs):
                asked = request.full_url.split("ids=")[1].split("&")[0].split("%7C")
                return Response(json.dumps({"entities": {qid: {"id": qid, "pad": "x" * 30} for qid in asked}}).encode())
            # The bound stands in for the compiler's 16 MiB: the whole chunk passes it, a half does not.
            with patch("tools.landmark_capture.MAX_JSON_SOURCE", 200):
                with patch("tools.landmark_capture.urlopen", side_effect=answer):
                    made = entities.fetch(chunk, "entities", "info")
            self.assertEqual([ids for ids, _, _ in made], [["Q38", "Q39"], ["Q40", "Q142"]], "halves, in order")
            for ids, path, values in made:
                self.assertEqual(sorted(values), sorted(ids))
                self.assertLessEqual((Path(directory) / path).stat().st_size, 200)
            # Its own response came back whole, so only `split` tells the manifest to drop it.
            self.assertEqual(len(entities.split), 1)
            self.assertIn(entities.split[0], [source["path"] for source in semantic_sources(capture.outcomes())])

    def test_a_single_identity_that_cannot_fit_is_not_split_forever(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            entities = Entities(capture)
            body = json.dumps({"entities": {"Q1": {"id": "Q1", "pad": "x" * 500}}}).encode()
            with patch("tools.landmark_capture.MAX_JSON_SOURCE", 10):
                with patch("tools.landmark_capture.urlopen", return_value=Response(body)) as request:
                    made = entities.fetch(["Q1"], "entities", "info")
            self.assertEqual(request.call_count, 1)
            self.assertEqual(entities.split, [])
            self.assertEqual(made[0][2]["Q1"]["id"], "Q1")

    def test_api_error_keeps_original_bytes_but_is_a_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            with patch("tools.landmark_capture.urlopen", return_value=Response(b'{"error":{"code":"nosuchentity"}}')):
                self.assertIsNone(capture.json("query.json", Response.url))
            outcome = capture.outcomes()[0]
            self.assertEqual(outcome["status"], "invalid-response")
            self.assertIn("sha256", outcome)
            self.assertEqual(semantic_sources([outcome]), [])
            capture.retry_failed()
            with patch("tools.landmark_capture.urlopen", return_value=Response(b'{"valid":true}')):
                self.assertEqual(capture.json("query.json", Response.url), {"valid": True})
            archived = json.loads(next((Path(directory) / "attempts").glob("*.json")).read_text())
            original = (Path(directory) / archived["archived_response_path"]).read_bytes()
            self.assertEqual(digest(original), outcome["sha256"])
            self.assertEqual(len(semantic_sources(capture.outcomes())), 1)

    def test_selection_refuses_a_rebuilt_compiler_and_policy_mismatch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "obc-bake"
            executable.write_bytes(b"first binary")
            def compile(argv, **_kwargs):
                (Path(argv[-1]) / "content.json").write_text(json.dumps({"candidate_qids": ["Q1"], "category_policy_sha256": "policy", "policy_sha256": "compiler-policy"}))
            with patch("tools.landmark_capture.subprocess.run", side_effect=compile):
                result = select_candidates(executable, root / "manifest", root / "boundary", "policy")
                self.assertEqual(result["compiler_sha256"], digest(b"first binary"))
                with self.assertRaisesRegex(ValueError, "discovery policy differs"):
                    select_candidates(executable, root / "manifest", root / "boundary", "different")
            def rebuild(argv, **kwargs):
                compile(argv, **kwargs)
                executable.write_bytes(b"second binary")
            with patch("tools.landmark_capture.subprocess.run", side_effect=rebuild):
                with self.assertRaisesRegex(ValueError, "compiler changed"):
                    select_candidates(executable, root / "manifest", root / "boundary", "policy")

    def test_best_rank_does_not_resurrect_a_normal_value(self):
        def claim(rank, value=None):
            return {"rank": rank, "mainsnak": {"datavalue": {"value": value}} if value else {"snaktype": "novalue"}}
        entity = {"claims": {"P18": [claim("normal", "Old.jpg"), claim("preferred", "New.jpg"), claim("deprecated", "Bad.jpg")]}}
        self.assertEqual(claim_values(entity, "P18"), ["New.jpg"])
        entity["claims"]["P18"][1] = claim("preferred")
        self.assertEqual(claim_values(entity, "P18"), [])

    def test_class_redirect_keeps_the_canonical_identity_in_the_closure(self):
        canonical = {"id": "Q2", "claims": {"P279": [{"mainsnak": {"datavalue": {"value": {"id": "Q3"}}}}]}}
        redirected = dict(canonical, redirects={"from": "Q1", "to": "Q2"})
        capture = Mock()
        capture.json.side_effect = [{"entities": {"Q2": canonical}}, {"entities": {"Q1": redirected}}]
        value = entity(capture, "Q1", "classes")
        self.assertEqual(class_parents(value, "Q1"), ["Q2"])
        self.assertEqual(class_parents(canonical, "Q2"), ["Q3"])
        with self.assertRaisesRegex(ValueError, "invalid class redirect"):
            class_parents(value, "Q4")

    def test_geographic_queries_include_boundary_without_country_filter(self):
        boundary = {"type": "Polygon", "coordinates": [[[5, 45], [11, 45], [11, 48], [5, 48], [5, 45]]]}
        bounds = bbox(boundary)
        self.assertEqual(bounds, (5, 45, 11, 48))
        sparql = query("Q133056", bounds)
        self.assertNotIn("P17", sparql)
        self.assertIn("wdt:P31/wdt:P279*", sparql)
        self.assertIn(">=45", sparql)
        self.assertIn("<=48", sparql)
        with self.assertRaises(ValueError):
            query("Q1 }", bounds)

    def test_lead_image_uses_exact_commons_filename_before_heading(self):
        parser = LeadImage()
        parser.feed('<a class="mw-file-description" href="/wiki/File:Outside.jpg"></a><div id="mw-content-text"><a class="mw-file-description" href="/wiki/File:Example_%C3%A9.jpg"><img src="//upload.wikimedia.org/wikipedia/commons/thumb/a/a1/Example_%C3%A9.jpg/240px-Example.jpg"></a><h2>History</h2><a class="mw-file-description" href="/wiki/File:Other.jpg"></a></div>')
        self.assertEqual(parser.filename, "Example é.jpg")
        self.assertEqual(parser.status, "commons")

    def test_local_lead_does_not_select_a_same_named_commons_file(self):
        parser = LeadImage()
        parser.feed('<div id="mw-content-text"><a class="mw-file-description" href="/wiki/File:Local.jpg"><img src="//upload.wikimedia.org/wikipedia/en/a/a1/Local.jpg"></a><a class="mw-file-description" href="/wiki/File:Later.jpg"><img src="//upload.wikimedia.org/wikipedia/commons/a/a1/Later.jpg"></a><h2>History</h2></div>')
        self.assertIsNone(parser.filename)
        self.assertEqual(parser.status, "unsupported-repository")


if __name__ == "__main__":
    unittest.main()


class LocaleCaptureTests(unittest.TestCase):
    def test_administrative_cycle_is_bounded_and_country_is_captured(self):
        from tools.landmark_capture import capture_locales, LANGUAGES
        self.assertEqual(LANGUAGES, ("en", "de", "fr", "es"))
        def claim(qid):
            return {"mainsnak": {"datavalue": {"value": {"id": qid}}}}
        place = {"claims": {"P131": [claim("Q10")], "P17": [claim("Q20")]}}
        admin = {"claims": {"P131": [claim("Q10")]}}
        with patch("tools.landmark_capture.entity", return_value=admin) as fetch:
            capture_locales(Mock(), place)
        self.assertEqual([call.args[1:] for call in fetch.call_args_list], [("Q20", "locales"), ("Q10", "locales")])
