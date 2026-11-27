import importlib.util
import sys
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).parents[1] / "dream" / "collect.py"
SPEC = importlib.util.spec_from_file_location("dream_collect", MODULE_PATH)
collect = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = collect
SPEC.loader.exec_module(collect)

NOW = datetime(2026, 11, 27, 4, 31, tzinfo=UTC)
REPO = ("owner", "repo")


def pull(number, merged, head="feature", association="OWNER", body="", comments=(), reviews=()):
    return {
        "number": number, "mergedAt": merged, "updatedAt": merged, "headRefName": head,
        "title": f"title {number}", "body": body, "additions": 5, "deletions": 2,
        "authorAssociation": association, "author": {"login": "someone"},
        "files": {"totalCount": 1, "nodes": [{"path": "tools/x.py", "additions": 5, "deletions": 2}]},
        "comments": {"nodes": list(comments)},
        "reviews": {"nodes": list(reviews)},
    }


def text(association, body):
    return {"authorAssociation": association, "author": {"login": "a"}, "body": body,
            "state": "COMMENTED", "path": "tools/x.py", "comments": {"nodes": []}}


class CollectTests(unittest.TestCase):
    def run_collect(self, pulls, dream=(), owner="User", since=None):
        """Collect against canned GitHub answers; returns the result and the written files."""
        by_number = {p["number"]: p for p in pulls}

        def graphql(query, **variables):
            if query is collect.MARKER_QUERY:
                return {"repository": {"owner": {"__typename": owner}, "pullRequests": {"nodes": list(dream)}}}
            if query is collect.LIST_QUERY:
                page = {"hasNextPage": False, "endCursor": None}
                return {"repository": {"pullRequests": {"pageInfo": page, "nodes": pulls}}}
            return {"repository": {"pullRequest": by_number[variables["number"]]}}

        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(collect, "graphql", graphql):
            out = Path(tmp) / "input"
            result = collect.collect(REPO, out, since, NOW)
            files = {p.name: p.read_text() for p in out.iterdir()} if out.exists() else {}
        return result, files

    def test_only_owner_and_collaborator_are_trusted(self):
        for association, expected in [("OWNER", True), ("COLLABORATOR", True), ("CONTRIBUTOR", False),
                                      ("MEMBER", False), ("NONE", False), (None, False)]:
            self.assertIs(collect.trusted({"authorAssociation": association}), expected, association)

    def test_text_of_other_authors_is_left_out_while_paths_and_stats_stay(self):
        review = text("OWNER", "review kept")
        review["comments"]["nodes"] = [text("NONE", "review comment dropped"), text("COLLABORATOR", "inline kept")]
        pulls = [
            pull(2, "2026-11-26T00:00:00Z", association="CONTRIBUTOR", body="body dropped\n\nLearned: dropped",
                 comments=[text("OWNER", "comment kept")]),
            pull(1, "2026-11-25T00:00:00Z", body="body kept",
                 comments=[text("NONE", "comment dropped")], reviews=[review, text("CONTRIBUTOR", "review dropped")]),
        ]
        _, files = self.run_collect(pulls)
        everything = "".join(files.values())
        self.assertNotIn("dropped", everything)
        self.assertNotIn("title 2", everything)
        for kept in ("title 1", "body kept", "comment kept", "review kept", "inline kept"):
            self.assertIn(kept, everything)
        for name in ("1.md", "2.md"):
            self.assertIn("+5 -2 tools/x.py", files[name])
        self.assertIn("- 2 +5 -2 tools:", files["index.md"])

    def test_an_organization_stops_before_any_output(self):
        with self.assertRaises(SystemExit) as stop:
            self.run_collect([pull(1, "2026-11-25T00:00:00Z")], owner="Organization")
        self.assertIn("organization", str(stop.exception))

    def test_the_window_starts_at_the_last_merged_dream_pull_request(self):
        dream = [
            {"number": 9, "mergedAt": "2026-11-20T00:00:00Z", "isCrossRepository": False,
             "body": "Dream window: 2026-11-13T00:00:00Z to 2026-11-20T00:00:00Z\r\n"},
            {"number": 8, "mergedAt": "2026-11-13T00:00:00Z", "isCrossRepository": False,
             "body": "Dream window: 2026-11-06T00:00:00Z to 2026-11-13T00:00:00Z"},
            {"number": 10, "mergedAt": "2026-11-26T00:00:00Z", "isCrossRepository": True,
             "body": "Dream window: 2026-11-19T00:00:00Z to 2026-11-26T00:00:00Z"},
        ]
        pulls = [pull(2, "2026-11-21T00:00:00Z"), pull(1, "2026-11-19T00:00:00Z")]
        (count, since, through), files = self.run_collect(pulls, dream=dream, since="2026-11-01T00:00:00Z")
        self.assertEqual((count, since, through), (1, "2026-11-20T00:00:00Z", "2026-11-27T04:31:00Z"))
        self.assertEqual(sorted(files), ["2.md", "index.md"])

    def test_without_a_dream_pull_request_the_window_starts_at_the_input_or_a_week_ago(self):
        pulls = [pull(1, "2026-11-19T00:00:00Z")]
        (count, since, _), _ = self.run_collect(pulls, since="2026-11-18T00:00:00Z")
        self.assertEqual((count, since), (1, "2026-11-18T00:00:00Z"))
        (count, since, _), files = self.run_collect(pulls)
        self.assertEqual((count, since), (0, "2026-11-20T04:31:00Z"))
        self.assertEqual(sorted(files), ["index.md"])

    def test_a_dream_pull_request_is_not_in_the_input(self):
        pulls = [pull(2, "2026-11-26T00:00:00Z", head="dream/pages"),
                 pull(3, "2026-11-26T00:00:00Z", head="dream/review-77"), pull(1, "2026-11-25T00:00:00Z")]
        (count, _, _), files = self.run_collect(pulls)
        self.assertEqual(count, 1)
        self.assertEqual(sorted(files), ["1.md", "index.md"])

    def test_the_learned_section_survives_the_cut_of_a_long_body(self):
        body = "x" * 10_000 + "\nLearned: early note\n" + "y" * 10 + "\nLearned:\n- keep `this`\n---\n_footer_\n"
        (_, _, _), files = self.run_collect([pull(1, "2026-11-25T00:00:00Z", body=body)])
        self.assertIn("```text\nLearned:\n- keep `this`\n```", files["1.md"])
        self.assertIn("(cut at 4,000 of", files["1.md"])
        self.assertIn("; Learned:", files["index.md"])

    def test_a_quote_cannot_be_closed_by_the_text_inside(self):
        quoted = collect.quote("## Heading\n````\nnot the end\n````", 1_000)
        self.assertTrue(quoted.startswith("`````text\n") and quoted.endswith("\n`````\n"))


if __name__ == "__main__":
    unittest.main()
