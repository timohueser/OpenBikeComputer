import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "dream" / "publish.py"
SPEC = importlib.util.spec_from_file_location("dream_publish", MODULE_PATH)
publish = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = publish
SPEC.loader.exec_module(publish)

MESSAGE = "AGENT-TEXT: @someone Closes #1\n````\nnot the end\n"
OK = {"is_error": False, "subtype": "success", "result": MESSAGE, "total_cost_usd": 12.5}
TREE = {
    "AGENTS.md": "index\n",
    "tools/README.md": "tools\n",
    "tools/x.py": "",
    "firmware/README.md": "firmware\n",
    "firmware/obc-render/README.md": "render\n",
}


class FakeGh:
    """Records each call and answers like `gh` for a repository with no dream issue."""

    def __init__(self, open_prs=()):
        self.calls = []
        self.open_prs = list(open_prs)

    def __call__(self, *args, stdin=None):
        self.calls.append((args, stdin))
        if args[:2] == ("api", "user"):
            return json.dumps({"login": "owner", "id": 42, "name": "Owner"})
        if args[:2] == ("issue", "list"):
            return "[]"
        if args[:2] == ("issue", "create"):
            return "ISSUE\n"
        if args[:2] == ("pr", "list"):
            return json.dumps(self.open_prs)
        if args[:2] == ("pr", "create"):
            return f"PR {args[args.index('--head') + 1]}\n"
        return ""

    def verbs(self):
        return [args[:2] for args, _ in self.calls]

    def body(self, verb, head=None):
        return next(s for a, s in self.calls if a[:2] == verb and (head is None or head in a))


def git(*args, cwd=None):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


class PublishTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        base = Path(tmp.name)
        self.root, self.remote, self.art, self.outside = base / "wt", base / "remote.git", base / "art", base / "outside"
        self.outside.mkdir()
        for name, text in TREE.items():
            (self.root / name).parent.mkdir(parents=True, exist_ok=True)
            (self.root / name).write_text(text)
        (self.root / "linked").symlink_to(self.outside)
        git("init", "-q", "--bare", "-b", "develop", str(self.remote))
        git("init", "-q", "-b", "develop", cwd=self.root)
        git("add", "-A", cwd=self.root)
        git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base", cwd=self.root)
        git("remote", "add", "origin", str(self.remote), cwd=self.root)
        git("push", "-q", "origin", "develop", cwd=self.root)

    def run_publish(self, pages, result=OK, gh=None, check=None, since="2026-11-01T00:00:00Z"):
        result_path = self.art.parent / "result.json"
        if result is not None:
            result_path.write_text(result if isinstance(result, str) else json.dumps(result))
        for name, text in (pages or {}).items():
            (self.art / name).parent.mkdir(parents=True, exist_ok=True)
            (self.art / name).write_text(text)
        if pages is not None:
            self.art.mkdir(exist_ok=True)
        self.gh = gh or FakeGh()
        return publish.publish(
            self.root, result_path, self.art, since, "2026-11-08T00:00:00Z", "77", "RUN",
            gh=self.gh, check=check or (lambda: []),
        )

    def branches(self):
        return set(git("--git-dir", str(self.remote), "for-each-ref", "--format=%(refname:short)", "refs/heads").split())

    def files(self, branch):
        return git("--git-dir", str(self.remote), "diff", "--name-only", "develop", branch).split()

    def test_area_pages_merge_on_their_own_and_the_rest_waits(self):
        code = self.run_publish({
            "AGENTS.md": "AGENT-TEXT index\n",
            "tools/README.md": "AGENT-TEXT tools\nmore\n",
            "firmware/README.md": "AGENT-TEXT firmware\n",
            "firmware/obc-render/README.md": "AGENT-TEXT render\n",
        })
        self.assertEqual(code, 0)
        self.assertEqual(self.branches(), {"develop", "dream/pages", "dream/review-77"})
        self.assertEqual(self.files("dream/pages"), ["firmware/README.md", "tools/README.md"])
        self.assertEqual(self.files("dream/review-77"), ["AGENTS.md", "firmware/obc-render/README.md"])
        merges = [args for args, _ in self.gh.calls if args[:2] == ("pr", "merge")]
        self.assertEqual(merges, [("pr", "merge", "PR dream/pages", "--auto", "--merge")])
        author = git("--git-dir", str(self.remote), "log", "-1", "--format=%an <%ae>", "dream/pages")
        self.assertEqual(author.strip(), "Owner <42+owner@users.noreply.github.com>")

        area = self.gh.body(("pr", "create"), "dream/pages")
        self.assertIn("Dream window: 2026-11-01T00:00:00Z to 2026-11-08T00:00:00Z", area.splitlines())
        self.assertIn("| `tools/README.md` | 1 | 2 |", area)
        self.assertIn("$12.50", area)
        self.assertNotIn("AGENT-TEXT", area)
        self.assertIn(publish.fence(MESSAGE), self.gh.body(("pr", "create"), "dream/review-77"))

    def test_only_page_files_reach_a_branch(self):
        (self.outside / "README.md").write_text("outside\n")
        (self.art / "firmware").mkdir(parents=True)
        (self.art / "firmware/README.md").symlink_to(self.outside / "README.md")
        (self.art / "firmware/obc-render").symlink_to(self.outside)
        code = self.run_publish({
            "tools/README.md": "new\n",
            "tools/x.py": "print('agent')\n",
            "tools/notes.md": "notes\n",
            "linked/README.md": "escapes through a link of the tree\n",
            "node_modules/README.md": "third party\n",
            ".git/README.md": "inside git\n",
        })
        self.assertEqual(code, 0)
        self.assertEqual(self.branches(), {"develop", "dream/pages"})
        self.assertEqual(self.files("dream/pages"), ["tools/README.md"])
        self.assertEqual((self.outside / "README.md").read_text(), "outside\n")
        self.assertFalse((self.root / ".git/README.md").exists())

    def test_a_failing_area_page_moves_to_review_and_the_others_merge(self):
        def check():
            return [f"{f}: BROKEN `x/y`" for f in TREE if "BROKEN" in (self.root / f).read_text()]

        code = self.run_publish({"tools/README.md": "BROKEN\n", "firmware/README.md": "fine\n"}, check=check)
        self.assertEqual(code, 0)
        self.assertEqual(self.files("dream/pages"), ["firmware/README.md"])
        self.assertEqual(self.files("dream/review-77"), ["tools/README.md"])
        self.assertIn(publish.fence("tools/README.md: BROKEN `x/y`"), self.gh.body(("pr", "create"), "dream/review-77"))

    def test_a_failed_run_publishes_nothing_and_writes_to_the_issue(self):
        page = {"tools/README.md": "new\n"}
        cases = {
            "error with subtype success": dict(pages=page, result={**OK, "is_error": True}),
            "no result": dict(pages=page, result=None),
            "not JSON": dict(pages=page, result="{"),
            "no pages": dict(pages=None),
            "no window": dict(pages=page, since=""),
            "check fails elsewhere": dict(pages=page, check=lambda: ["specs/x.md: link to y does not resolve"]),
        }
        for name, case in cases.items():
            with self.subTest(name):
                self.setUp()
                self.assertEqual(self.run_publish(**case), 1)
                self.assertEqual(self.branches(), {"develop"})
                self.assertNotIn(("pr", "create"), self.gh.verbs())
                self.assertIn(("issue", "comment"), self.gh.verbs())

    def test_the_cost_of_a_capped_run_reaches_the_issue(self):
        self.run_publish({}, result={**OK, "subtype": "error_max_budget_usd", "total_cost_usd": 35.1})
        body = self.gh.body(("issue", "comment"))
        self.assertIn("Cost: $35.10", body)
        self.assertIn("error_max_budget_usd", body)
        self.assertIn(publish.fence(MESSAGE), body)

    def test_an_open_area_pr_is_closed_first_and_a_review_pr_is_not(self):
        gh = FakeGh(open_prs=[{"number": 7, "isCrossRepository": False}, {"number": 8, "isCrossRepository": True}])
        self.run_publish({"tools/README.md": "new\n", "AGENTS.md": "new\n"}, gh=gh)
        closed = [args[2] for args, _ in gh.calls if args[:2] == ("pr", "close")]
        self.assertEqual(closed, ["7"])
        self.assertLess(gh.verbs().index(("pr", "close")), gh.verbs().index(("pr", "create")))
        listed = [args for args, _ in gh.calls if args[:2] == ("pr", "list")]
        self.assertTrue(all("dream/pages" in args for args in listed))

    def test_text_of_the_agent_cannot_close_its_fence(self):
        fenced = publish.fence(MESSAGE).splitlines()
        self.assertEqual(fenced[0], "`````text")
        self.assertEqual(fenced[-1], "`````")
        self.assertFalse(any(line.startswith("`````") for line in fenced[1:-1]))


if __name__ == "__main__":
    unittest.main()
