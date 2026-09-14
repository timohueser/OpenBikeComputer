from concurrent.futures import ThreadPoolExecutor
from io import BytesIO
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

from tools.landmark_capture import Capture, LeadImage, bbox, claim_values, query


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
            error = HTTPError(Response.url, 429, "Too many requests", {}, None)
            with patch("tools.landmark_capture.urlopen", side_effect=error) as request:
                first = capture.fetch("query.json", Response.url)
                self.assertEqual(capture.fetch("query.json", Response.url), first)
                request.assert_called_once()
            self.assertEqual(first["status"], "http-error")
            self.assertEqual(first["http_status"], 429)
            self.assertFalse((Path(directory) / "query.json").exists())

    def test_api_error_keeps_original_bytes_but_is_a_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Capture(Path(directory), interval=0)
            with patch("tools.landmark_capture.urlopen", return_value=Response(b'{"error":{"code":"maxlag"}}')):
                self.assertIsNone(capture.json("query.json", Response.url))
            outcome = capture.outcomes()[0]
            self.assertEqual(outcome["status"], "invalid-response")
            self.assertIn("sha256", outcome)

    def test_best_rank_does_not_resurrect_a_normal_value(self):
        def claim(rank, value=None):
            return {"rank": rank, "mainsnak": {"datavalue": {"value": value}} if value else {"snaktype": "novalue"}}
        entity = {"claims": {"P18": [claim("normal", "Old.jpg"), claim("preferred", "New.jpg"), claim("deprecated", "Bad.jpg")]}}
        self.assertEqual(claim_values(entity, "P18"), ["New.jpg"])
        entity["claims"]["P18"][1] = claim("preferred")
        self.assertEqual(claim_values(entity, "P18"), [])

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
        parser.feed('<a class="mw-file-description" href="/wiki/File:Outside.jpg"></a><div id="mw-content-text"><a class="mw-file-description" href="/wiki/File:Example_%C3%A9.jpg"></a><h2>History</h2><a class="mw-file-description" href="/wiki/File:Other.jpg"></a></div>')
        self.assertEqual(parser.filename, "Example é.jpg")


if __name__ == "__main__":
    unittest.main()
