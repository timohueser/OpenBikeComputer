from concurrent.futures import ThreadPoolExecutor
from io import BytesIO
from pathlib import Path
import json
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.error import HTTPError

from tools.landmark_capture import BACKOFF_ATTEMPTS, BATCH, PHOTO_SOURCES, Capture, Entities, LeadImage, acquire_requested_photos, batches, bbox, capture_assets, category_coverage_complete, category_files, claim_values, class_parents, digest, entity, image_metadata_url, photo_bytes, photo_metadata, query, retry_after, select_candidates, semantic_sources


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
        self.assertIn(f"maxlag={MAXLAG}", image_metadata_url("A.jpg"))

    def test_a_commons_category_listing_waits_like_every_other_request(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            error = HTTPError(Response.url, 429, "Too many requests", {"Retry-After": "9"}, None)
            with patch("tools.landmark_capture.urlopen", side_effect=error) as request:
                with patch("tools.landmark_capture.time.sleep") as sleep:
                    pages, status, members = category_files(capture, "Category:Alpspitz")
            self.assertEqual((status, members), ("acquisition-failed", []))
            self.assertEqual(request.call_count, BACKOFF_ATTEMPTS, "the back-off is bounded")
            self.assertGreater(max(call.args[0] for call in sleep.call_args_list), 8)
            self.assertTrue(pages[0]["path"].startswith("categories/"))
            asked = request.call_args.args[0].full_url
            self.assertIn("list=categorymembers", asked)
            self.assertIn("cmlimit=max", asked)

    def test_commons_category_follows_every_continuation_page(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            token = {"cmcontinue": "file|next|7", "continue": "-||"}
            first = {
                "continue": token,
                "query": {"categorymembers": [{"title": f"File:Photo {number}.jpg"} for number in range(500)]},
            }
            second = {"query": {"categorymembers": [{"title": "File:Photo 500.jpg"}]}}
            with patch(
                "tools.landmark_capture.urlopen",
                side_effect=[Response(json.dumps(first).encode()), Response(json.dumps(second).encode())],
            ) as request:
                pages, status, members = category_files(capture, "Category:Rhine Falls")
            self.assertEqual(status, "captured")
            self.assertEqual(len(members), 501)
            self.assertEqual([page["continuation"] for page in pages], [None, token])
            self.assertEqual(request.call_count, 2)
            self.assertIn("cmcontinue=file%7Cnext%7C7", request.call_args_list[1].args[0].full_url)
            outcomes = {outcome["path"]: outcome for outcome in capture.outcomes()}
            self.assertEqual(set(outcomes), {page["path"] for page in pages})
            self.assertTrue(all("sha256" in outcome for outcome in outcomes.values()))

    def test_commons_category_resumes_after_page_two_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            token = {"cmcontinue": "file|next|7", "continue": "-||"}
            first = {"continue": token, "query": {"categorymembers": [{"title": "File:First.jpg"}]}}
            failure = HTTPError(Response.url, 404, "Not found", {}, None)
            with patch(
                "tools.landmark_capture.urlopen", side_effect=[Response(json.dumps(first).encode()), failure]
            ):
                pages, status, members = category_files(capture, "Category:Rhine Falls")
            self.assertEqual((status, members), ("acquisition-failed", []))
            self.assertEqual(len(pages), 2)
            outcomes = {outcome["path"]: outcome["status"] for outcome in capture.outcomes()}
            self.assertEqual(outcomes, {pages[0]["path"]: "ok", pages[1]["path"]: "http-error"})

            capture.retry_failed()
            second = {"query": {"categorymembers": [{"title": "File:Second.jpg"}]}}
            with patch("tools.landmark_capture.urlopen", return_value=Response(json.dumps(second).encode())) as request:
                resumed_pages, status, members = category_files(capture, "Category:Rhine Falls")
            self.assertEqual((status, members), ("captured", ["First.jpg", "Second.jpg"]))
            self.assertEqual(resumed_pages, pages)
            request.assert_called_once()

    def test_incomplete_commons_category_contributes_no_partial_asset(self):
        value = {
            "sitelinks": {"enwiki": {"title": "Example"}},
            "claims": {"P373": [{"mainsnak": {"datavalue": {"value": "Example"}}}]},
        }
        capture = Mock()
        capture.json.side_effect = [
            {
                "continue": {"cmcontinue": "file|next|7", "continue": "-||"},
                "query": {"categorymembers": [{"title": "File:Partial.jpg"}]},
            },
            None,
        ]
        with patch("tools.landmark_capture.capture_locales"), patch(
            "tools.landmark_capture.article", return_value=(None, None, "article-missing")
        ), patch("tools.landmark_capture.photo_metadata") as metadata:
            place = capture_assets(capture, "Q5", value)
        self.assertEqual(place["images"], [])
        self.assertFalse(place["commons_categories"][0]["complete"])
        self.assertFalse(category_coverage_complete([place]))
        metadata.assert_not_called()

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

    def test_photo_metadata_carries_categories_and_structured_data_but_no_original(self):
        capture = Mock()
        capture.json.side_effect = [
            {"query": {"pages": {"7": {"pageid": 7, "title": "File:A.jpg", "imageinfo": [{"mime": "image/jpeg", "size": 10, "url": "https://example.test/a.jpg"}]}}}},
            {"entities": {"M7": {"statements": {"P180": []}}}},
        ]
        record, status = photo_metadata(capture, "A.jpg")
        self.assertEqual(status, "captured")
        self.assertNotIn("path", record, "an original costs megabytes and waits for the compiler")
        capture.fetch.assert_not_called()
        self.assertIn("prop=imageinfo%7Ccategories", capture.json.call_args_list[0].args[1])
        self.assertIn("ids=M7", capture.json.call_args_list[1].args[1])
        self.assertEqual(record["depicts_path"], f"images/{digest(b'A.jpg')}-mediainfo.json")

    def test_acquisition_asks_again_until_the_compiler_needs_nothing_new(self):
        capture = Mock()
        capture.json.return_value = {"query": {"pages": {"7": {"pageid": 7, "title": "File:X.jpg", "imageinfo": [{"mime": "image/jpeg", "size": 10, "url": "https://example.test/x.jpg"}]}}}}
        # The first two originals fail to download; the third arrives.
        capture.fetch.side_effect = [{"status": "http-error"}, {"status": "transport-error"}, {"status": "ok"}]
        places = [{"qid": "Q5", "outcomes": [], "images": [
            {"filename": f"{name}.jpg", "metadata_path": f"images/{name}.json", "source": "P18"} for name in "abc"]}]
        def request(name):
            return {"qid": "Q5", "filename": f"{name}.jpg", "metadata_path": f"images/{name}.json"}
        rounds = [{"photo_requests": [request("a"), request("b")]}, {"photo_requests": [request("c")]}, {}]
        def compile_once(command, **_kwargs):
            self.assertIn("--photo-requests", command)
            Path(command[-1], "content.json").write_text(json.dumps(rounds.pop(0)))
        written = []
        with patch("tools.landmark_capture.subprocess.run", side_effect=compile_once) as compiler:
            acquired = acquire_requested_photos(capture, Path("obc-bake"), "landmark-content", "content.json", Path("m.json"), Path("b.geojson"), places, lambda: written.append(True))
        self.assertEqual((acquired, compiler.call_count), (1, 3), "one more round after the failures, then done")
        self.assertEqual([image.get("path") for image in places[0]["images"]], [None, None, f"images/{digest(b'c.jpg')}.jpg"])
        self.assertEqual(capture.fetch.call_count, 3, "no candidate is downloaded twice")
        self.assertEqual(len(written), 4, "the manifest is written before every compile and after the bytes")

    def test_a_compiled_catalogue_that_needs_nothing_is_not_an_error(self):
        places = [{"qid": "Q5", "outcomes": [], "images": []}]
        def compile_once(command, **_kwargs):
            Path(command[-1], "content.json").write_text(json.dumps({}))
        with patch("tools.landmark_capture.subprocess.run", side_effect=compile_once):
            acquired = acquire_requested_photos(Mock(), Path("obc-bake"), "landmark-content", "content.json", Path("m.json"), Path("b.geojson"), places, lambda: None)
        self.assertEqual(acquired, 0)

    def test_later_request_rounds_only_revisit_the_preceding_qids(self):
        capture = Mock()
        capture.json.return_value = {"query": {"pages": {"7": {"pageid": 7, "title": "File:X.jpg", "imageinfo": [{"mime": "image/jpeg", "size": 10, "url": "https://example.test/x.jpg"}]}}}}
        capture.fetch.side_effect = [{"status": "http-error"}, {"status": "ok"}, {"status": "ok"}]
        places = [
            {"qid": "Q5", "outcomes": [], "images": [
                {"filename": "shared.jpg", "metadata_path": "images/shared.json", "source": "P18"},
                {"filename": "fallback.jpg", "metadata_path": "images/fallback.json", "source": "P18"},
            ]},
            {"qid": "Q6", "outcomes": [], "images": [
                {"filename": "shared.jpg", "metadata_path": "images/shared.json", "source": "P18"},
            ]},
            {"qid": "Q7", "outcomes": [], "images": [
                {"filename": "settled.jpg", "metadata_path": "images/settled.json", "source": "P18", "path": "images/settled.jpg"},
            ]},
        ]
        rounds = [
            [
                {"qid": "Q5", "filename": "shared.jpg", "metadata_path": "images/shared.json"},
                {"qid": "Q6", "filename": "shared.jpg", "metadata_path": "images/shared.json"},
            ],
            [{"qid": "Q5", "filename": "fallback.jpg", "metadata_path": "images/fallback.json"}],
            [],
        ]
        filters = []
        def compile_once(command, **_kwargs):
            self.assertEqual(command[1], "landmark-photo-requests")
            qids = json.loads(Path(command[command.index("--qids") + 1]).read_text()) if "--qids" in command else None
            filters.append(qids)
            Path(command[command.index("--out") + 1], "photo-requests.json").write_text(
                json.dumps({"schema": 1, "requests": rounds.pop(0)})
            )
        with patch("tools.landmark_capture.subprocess.run", side_effect=compile_once):
            acquired = acquire_requested_photos(
                capture, Path("obc-bake"), "landmark-photo-requests", "photo-requests.json",
                Path("m.json"), Path("b.geojson"), places, lambda: None,
            )
        self.assertEqual(acquired, 2)
        self.assertEqual(filters, [None, ["Q5", "Q6"], ["Q5"]])
        self.assertNotIn("Q7", filters[1], "a settled place is never prepared in a later round")
        self.assertIsNone(places[0]["images"][0].get("path"), "a failed shared original stays absent for this QID")
        self.assertIsNotNone(places[1]["images"][0].get("path"), "the other QID owns its acquired-path update")

    def test_a_peak_entity_with_no_supported_sitelink_still_reaches_its_photos(self):
        value = {"labels": {"de": {"value": "Schafberg"}}, "sitelinks": {"cebwiki": {"title": "Schafberg"}},
                 "claims": {"P18": [{"mainsnak": {"datavalue": {"value": "Peak.jpg"}}}]}}
        metadata = json.dumps({"query": {"pages": {"1": {"pageid": 1, "title": "File:Peak.jpg", "imageinfo": [
            {"mime": "image/jpeg", "size": 10, "url": "https://example.test/Peak.jpg", "sha1": "a", "extmetadata": {}}]}}}}).encode()
        for photo_without_text, images in ((False, []), (True, ["Peak.jpg"])):
            with tempfile.TemporaryDirectory() as directory:
                capture = Capture(Path(directory), interval=0)
                with patch("tools.landmark_capture.urlopen", return_value=Response(metadata)):
                    place = capture_assets(capture, "Q5", value, photo_without_text)
                self.assertEqual([image["filename"] for image in place["images"]], images)
                self.assertEqual(place["articles"], [], "an unsupported edition is never an article")
                self.assertIn(dict(asset="article", status="no-supported-sitelink"), place["outcomes"])
                self.assertEqual(place["name"], "Schafberg")

    def test_candidate_pool_adds_view_claims_and_every_commons_category(self):
        def claims(**properties):
            return {prop: [{"mainsnak": {"datavalue": {"value": name}}} for name in names] for prop, names in properties.items()}
        value = {"sitelinks": {"enwiki": {"title": "Alpspitz"}},
                 "claims": claims(P18=["Lead.jpg"], P4291=["Panorama.jpg"], P373=["Alpspitz", "Alpspitz massif"])}
        capture = Mock()
        capture.json.return_value = {"query": {"categorymembers": [{"title": "File:In_category.jpg"}]}}
        for context in (patch("tools.landmark_capture.capture_locales"),
                        patch("tools.landmark_capture.article", return_value=(None, None, "article-missing")),
                        patch("tools.landmark_capture.photo_metadata", side_effect=lambda _, name: (dict(metadata_path="m", filename=name), "captured"))):
            self.addCleanup(context.stop)
            context.start()
        place = capture_assets(capture, "Q5", value)
        # A P18 claim no longer hides the category: ranking, not acquisition, decides between them.
        self.assertEqual([(i["source"], i["filename"]) for i in place["images"]],
                         [("P18", "Lead.jpg"), ("commons-category", "In category.jpg"), ("P4291", "Panorama.jpg")])
        self.assertEqual([c["title"] for c in place["commons_categories"]],
                         ["Category:Alpspitz", "Category:Alpspitz massif"])
        self.assertEqual(capture.json.call_count, 2, "one listing per P373 claim")
        self.assertEqual(PHOTO_SOURCES[0], "P18")


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
