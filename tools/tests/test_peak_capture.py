import unittest
from unittest.mock import Mock

from tools.peak_capture import resolve


class PeakCaptureTests(unittest.TestCase):
    def test_qid_redirect_and_direct_article_share_one_identity(self):
        capture = Mock()
        capture.json.side_effect = [
            {"entities": {"Q1": {"id": "Q2", "redirects": {"from": "Q1", "to": "Q2"}}}},
            {"query": {"redirects": [{"from": "Old", "to": "Mountain"}], "pages": {"3": {"pageid": 3, "title": "Mountain", "pageprops": {"wikibase_item": "Q2"}}}}},
        ]
        outcomes, identities = resolve(capture, {"node_id": 4, "tags": {"wikidata": "Q1", "wikipedia": "en:Old"}})
        self.assertEqual(identities, {"Q2": None})
        self.assertTrue(all(outcome["status"] == "resolved" for outcome in outcomes))
        self.assertIn("redirects=yes", capture.json.call_args_list[0].args[1])
        self.assertIn("redirects=1", capture.json.call_args_list[1].args[1])

    def test_unsupported_osm_edition_follows_explicit_supported_links_without_a_qid(self):
        capture = Mock()
        capture.json.side_effect = [
            {"query": {"pages": {"1": {"pageid": 1, "title": "Monte", "langlinks": [{"lang": "fr", "*": "Mont"}, {"lang": "en", "*": "Hill"}]}}}},
            {"query": {"pages": {"42": {"pageid": 42, "title": "Hill", "langlinks": [{"lang": "it", "*": "Monte"}, {"lang": "fr", "*": "Mont"}]}}}},
        ]
        outcomes, identities = resolve(capture, {"node_id": 5, "tags": {"wikipedia": "it:Monte"}})
        self.assertEqual(list(identities), ["wiki-en-42"])
        self.assertEqual(identities["wiki-en-42"]["sitelinks"], {"enwiki": {"title": "Hill"}, "frwiki": {"title": "Mont"}})
        self.assertEqual(outcomes[0]["canonical_language"], "en")
        self.assertTrue(outcomes[0]["canonical_path"].startswith("links/"))

    def test_unlinked_and_invalid_nodes_never_trigger_name_or_coordinate_queries(self):
        capture = Mock()
        for tags in ({"name": "Matterhorn"}, {"wikidata": "Q1;Q2"}, {"wikipedia": "https://example.test/Peak"}):
            _, identities = resolve(capture, {"node_id": 5, "tags": tags, "latitude": 1, "longitude": 2})
            self.assertEqual(identities, {})
        capture.json.assert_not_called()

    def test_missing_and_conflicting_links_are_retained_as_outcomes(self):
        capture = Mock()
        capture.json.side_effect = [
            {"entities": {"Q1": {"id": "Q1"}}},
            {"query": {"pages": {"2": {"pageid": 2, "title": "Hill", "pageprops": {"wikibase_item": "Q2"}}}}},
            None,
        ]
        outcomes, identities = resolve(capture, {"node_id": 1, "tags": {"wikidata": "Q1", "wikipedia": "en:Hill"}})
        self.assertEqual(set(identities), {"Q1", "Q2"})
        self.assertEqual(len(outcomes), 2)
        outcomes, identities = resolve(capture, {"node_id": 2, "tags": {"wikidata": "Q3"}})
        self.assertEqual(outcomes[0]["status"], "link_acquisition_failed")
        self.assertEqual(identities, {})
