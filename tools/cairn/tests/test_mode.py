import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parents[1]))
import mode  # noqa: E402

SOURCES = {"extensions": [".rs", ".py"], "files": ["tools/obc"]}
NAMES = {"repository-tooling": "Repository tooling", "routing": "Routing", "search": "Search"}


def git(folder, *args):
    subprocess.run(["git", "-C", str(folder), "-c", "user.name=t", "-c", "user.email=t@t", *args],
                   check=True, capture_output=True)


def repository(folder):
    git(folder, "init", "--quiet", "--initial-branch=topic")
    git(folder, "commit", "--quiet", "--allow-empty", "-m", "start")
    return folder


def issue(systems="routing (new: cairn), documentation", status="Status: approved", size="**Size:** M (2 systems)",
          plan="The plan."):
    body = f"## TL;DR\n\n{size}\n**Systems:** {systems}\n\n{plan}   \n"
    return f"{status}\nApproved: {mode.approval_hash(body)}\n{body}"


class HashTests(unittest.TestCase):
    def test_the_status_lines_and_trailing_whitespace_do_not_count(self):
        body = "Problem.\nThe plan.\n"
        self.assertEqual(mode.approval_hash(body), mode.approval_hash("Status: approved\nProblem.  \nThe plan.\n\n"))
        self.assertEqual(mode.approval_hash(body), mode.approval_hash("Problem.\nApproved: 0123456789ab\nThe plan."))
        self.assertNotEqual(mode.approval_hash(body), mode.approval_hash("Problem.\nAnother plan.\n"))
        self.assertRegex(mode.approval_hash(body), r"\A[0-9a-f]{12}\Z")


class LineTests(unittest.TestCase):
    def test_a_declaration_gives_the_line_that_parses_back(self):
        cases = {
            ("fix", None, None): "fix",
            ("fix", None, "a generated table"): "fix, over limit: a generated table",
            ("change", "#12", None): "change #12",
            ("change", "12", None): "change #12",
            ("prototype", None, None): "prototype",
            ("override", "a release fix", None): "override: a release fix",
        }
        for arguments, line in cases.items():
            declared = mode.declaration(*arguments)
            self.assertEqual(declared.line(), line)
            self.assertEqual(mode.parse(line), declared)

    def test_a_wrong_declaration_is_refused(self):
        for arguments in (("change", None, None), ("override", None, None), ("prototype", "x", None),
                          ("change", "12", "big"), ("fix", None, " ")):
            with self.assertRaises(ValueError, msg=arguments):
                mode.declaration(*arguments)

    def test_the_body_line_ignores_the_template_hint(self):
        template = "<!-- fix or change #N -->\nMode:\nRequirements: none\n"
        self.assertIsNone(mode.body_value(template))
        self.assertEqual(mode.body_value("Intro.\r\nMode: change #7\r\n"), "change #7")


class StoreTests(unittest.TestCase):
    def test_the_mode_belongs_to_the_branch(self):
        with tempfile.TemporaryDirectory() as folder:
            root = repository(Path(folder))
            self.assertIsNone(mode.declared(root))
            self.assertEqual(mode.declare(root, mode.Mode("change", 7)), "topic")
            self.assertEqual(mode.declared(root), "change #7")
            git(root, "switch", "--quiet", "-c", "other")
            self.assertIsNone(mode.declared(root))
            git(root, "switch", "--quiet", "--detach")
            with self.assertRaises(ValueError):
                mode.declare(root, mode.Mode("fix"))


class RuleTests(unittest.TestCase):
    def check(self, line, diff, body=None, limit=1000, epic=None):
        rules = {**mode.DEFAULTS, "fix": {"max_lines": limit}}
        return mode.violations(mode.parse(line), diff, rules, body, NAMES, epic)

    def test_a_fix_stays_inside_one_system_and_its_size(self):
        self.assertEqual(self.check("fix", mode.Diff(frozenset({"routing"}), lines=1000)), [])
        found = self.check("fix", mode.Diff(
            frozenset({"routing", "search"}), lines=1001, new_edges=frozenset({("routing", "search")}),
            headers=frozenset({"routing"}), specs=("specs/obcr-route.md",)))
        self.assertEqual(len(found), 5, found)
        # The over-limit reason lifts the size limit and nothing else.
        found = self.check("fix, over limit: a generated table", mode.Diff(
            frozenset({"routing"}), lines=5000, specs=("specs/obcr-route.md",)))
        self.assertEqual(found, ["a fix changes no spec, and this one changes specs/obcr-route.md"])

    def test_a_change_needs_an_approved_unchanged_issue_that_lists_its_systems(self):
        diff = mode.Diff(frozenset({"routing", "repository-tooling"}))
        self.assertEqual(self.check("change #5", diff, issue("routing, repository tooling")), [])
        self.assertEqual(self.check("change #5", diff, issue("Routing")),
                         ["issue #5 does not list repository-tooling on its Systems line"])
        self.assertEqual(self.check("change #5", diff, None), ["issue #5 could not be read"])
        edited = issue("routing, repository-tooling").replace("The plan.", "Another plan.")
        self.assertIn("changed after its approval", self.check("change #5", diff, edited)[0])
        unapproved = issue("routing, repository-tooling", status="Status: designed")
        refused = ["the first `Status:` line of issue #5 is not `Status: approved`"]
        self.assertEqual(self.check("change #5", diff, unapproved), refused)
        self.assertEqual(self.check("change #5", diff, unapproved.replace("The plan.", "The plan.\nStatus: approved")), refused)

    def test_the_systems_line_in_a_code_block_does_not_count(self):
        body = "```\n**Systems:** routing\n```\n**Systems:** search\n"
        self.assertEqual(mode.listed(body, NAMES), {"search"})

    def test_a_pull_request_cannot_approve_a_change(self):
        self.assertEqual(mode.issue_body(5, {"body": "Status: approved"}), "Status: approved")
        with self.assertRaisesRegex(ValueError, "#5 is a pull request"):
            mode.issue_body(5, {"body": "Status: approved", "pull_request": {"url": "x"}})

    def test_a_change_that_adds_a_dependency_changes_the_doc_of_its_user(self):
        edge = frozenset({("routing", "search")})
        body = issue("routing, search")
        self.assertEqual(self.check("change #5", mode.Diff(frozenset({"routing"}), new_edges=edge), body),
                         ["the change adds routing -> search, so it changes docs/design/routing.md too"])
        both = mode.Diff(frozenset({"routing"}), new_edges=edge, docs=frozenset({"routing"}))
        self.assertEqual(self.check("change #5", both, body), [])

    def test_an_override_passes_and_a_prototype_always_fails(self):
        wide = mode.Diff(frozenset(NAMES), lines=10**6, specs=("specs/x.md",))
        self.assertEqual(self.check("override: a hotfix", wide), [])
        self.assertTrue(mode.fails(mode.parse("prototype"), self.check("prototype", wide), frozenset(), set()))

    def test_a_violation_fails_only_when_the_diff_touches_an_approved_system(self):
        found = ["a violation"]
        self.assertFalse(mode.fails(mode.parse("fix"), found, frozenset({"routing"}), set()))
        self.assertFalse(mode.fails(mode.parse("fix"), found, frozenset({"routing"}), {"search"}))
        self.assertTrue(mode.fails(mode.parse("fix"), found, frozenset({"routing"}), {"routing"}))
        self.assertFalse(mode.fails(mode.parse("fix"), [], frozenset({"routing"}), {"routing"}))
        # A missing Mode line is a violation like any other.
        self.assertTrue(mode.fails(None, found, frozenset({"routing"}), {"routing"}))
        self.assertFalse(mode.fails(None, found, frozenset(), {"routing"}))


class SizeTests(unittest.TestCase):
    def size(self, **diff):
        return mode.size(mode.Diff(**{"touched": frozenset({"routing"}), **diff}), mode.DEFAULTS["size"])

    def check(self, diff, body):
        return mode.violations(mode.parse("change #5"), diff, mode.DEFAULTS, body, NAMES)

    def test_each_signal_moves_the_size_up(self):
        three = frozenset({"routing", "search", "repository-tooling"})
        self.assertEqual(self.size(lines=500), ("S", ["1 system", "500 changed lines outside tests"]))
        self.assertEqual(self.size(lines=501), ("M", ["501 changed lines outside tests"]))
        self.assertEqual(self.size(lines=1500)[0], "M")
        self.assertEqual(self.size(lines=1501), ("L", ["1,501 changed lines outside tests"]))
        self.assertEqual(self.size(touched=frozenset({"routing", "search"})), ("M", ["2 systems"]))
        self.assertEqual(self.size(touched=three)[0], "M")
        self.assertEqual(self.size(touched=three | {"documentation"}), ("L", ["4 systems"]))
        self.assertEqual(self.size(specs=("specs/obcr-route.md",)), ("M", ["a contract change in specs/obcr-route.md"]))
        self.assertEqual(self.size(new_edges=frozenset({("routing", "search")})), ("M", ["a new dependency routing -> search"]))
        self.assertEqual(self.size(new_docs=frozenset({"search"})), ("L", ["a new system doc for search"]))

    def test_a_change_larger_than_its_issue_is_a_note_and_a_missing_size_is_a_violation(self):
        body = issue("routing", size="**Size:** S (one system)")
        larger = mode.Diff(frozenset({"routing"}), lines=640)
        self.assertEqual(self.check(larger, body), [])
        self.assertEqual(mode.size_note(5, body, larger, mode.DEFAULTS["size"]),
                         "`change #5` is sized S, but the diff is M: 640 changed lines outside tests")
        self.assertIsNone(mode.size_note(5, body, mode.Diff(frozenset({"routing"}), lines=500), mode.DEFAULTS["size"]))
        self.assertEqual(self.check(mode.Diff(frozenset({"routing"})), issue("routing", size="")),
                         ["issue #5 has no `**Size:** S|M|L` line"])
        self.assertIsNone(mode.issue_size("**Size:**\nS\n"))
        self.assertIsNone(mode.issue_size("<!--\n**Size:** S\n-->\n"))
        self.assertIsNone(mode.issue_size("<!--\n**Size:** S\n"))


class EpicTests(unittest.TestCase):
    def check(self, sub, epic, number=7):
        diff = mode.Diff(frozenset({"routing"}))
        return mode.violations(mode.parse(f"change #{number}"), diff, mode.DEFAULTS, sub, NAMES, (5, epic))

    def test_a_sub_issue_passes_with_its_approved_epic_inside_its_scope(self):
        sub = "**Size:** S\n**Systems:** routing\n"
        self.assertEqual(self.check(sub, issue("routing, search", plan="- [ ] #7")), [])
        self.assertEqual(self.check(sub, issue("routing", status="Status: designed", plan="- #7")),
                         ["the first `Status:` line of issue #5 is not `Status: approved`"])
        self.assertEqual(self.check("**Size:** S\n**Systems:** routing, search\n", issue("routing", plan="- #7")),
                         ["issue #7 lists search on its Systems line, and its epic #5 does not"])

    def test_a_sub_issue_is_on_the_epic_list_and_one_pull_request(self):
        sub = "**Size:** S\n**Systems:** routing\n"
        for plan in ("The plan.", "- #80", "Depends on #8", "```\n- #8\n```", "<!-- - #8 -->", "<!--\n- #8"):
            self.assertEqual(self.check(sub, issue("routing", plan=plan), number=8),
                             ["issue #8 is not on the list of its epic #5"], plan)
        large = "**Size:** L\n**Systems:** routing\n"
        self.assertEqual(self.check(large, issue("routing", plan="- #8"), number=8),
                         ["issue #8 is sized L, but a sub-issue is one pull request: size S or M"])

    def test_the_epic_is_the_parent_from_the_sub_issue_relation(self):
        with mock.patch.object(mode, "api", side_effect=OSError("gh: No parent issue found (HTTP 404)")):
            self.assertIsNone(mode.fetch_epic(Path("."), 7))
        with mock.patch.object(mode, "api", side_effect=OSError("gh: Not Found (HTTP 404)")), self.assertRaises(OSError):
            mode.fetch_epic(Path("."), 7)
        with mock.patch.object(mode, "api", return_value={"number": 5, "body": "Epic."}):
            self.assertEqual(mode.fetch_epic(Path("."), 7), (5, "Epic."))
        with mock.patch.object(mode, "api", return_value={"number": 5, "body": "", "pull_request": {}}), \
                self.assertRaisesRegex(ValueError, "#5 is a pull request"):
            mode.fetch_epic(Path("."), 7)
        with mock.patch.object(mode, "api", return_value={"body": "Epic."}), \
                self.assertRaisesRegex(ValueError, "the parent of issue #7 has no number"):
            mode.fetch_epic(Path("."), 7)


class HookTests(unittest.TestCase):
    def blocks(self, root, path, cwd=None):
        event = {"tool_name": "Edit", "cwd": str(cwd or root), "tool_input": {"file_path": path}}
        return mode.hook_message(event, root, SOURCES) is not None

    def test_a_source_edit_waits_for_a_declared_mode(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / "repo"
            root.mkdir()
            repository(root)
            self.assertTrue(self.blocks(root, "src/new.rs"))
            self.assertTrue(self.blocks(root, str(root / "tools/obc")))
            self.assertTrue(mode.hook_message(
                {"tool_input": {"notebook_path": str(root / "a.py")}}, root, SOURCES))
            self.assertFalse(self.blocks(root, "README.md"))
            self.assertFalse(self.blocks(root, str(Path(folder) / "outside.rs")))
            self.assertFalse(self.blocks(root, "../outside.rs"))
            # Input of the wrong type passes, or falls back to the process folder, with no exception.
            self.assertIsNone(mode.hook_message({"tool_input": "src/new.rs"}, root, SOURCES))
            self.assertIsNone(mode.hook_message({"tool_input": ["src/new.rs"]}, root, SOURCES))
            self.assertTrue(mode.hook_message({"cwd": 7, "tool_input": {"file_path": str(root / "a.rs")}}, root, SOURCES))
            mode.declare(root, mode.Mode("fix"))
            self.assertFalse(self.blocks(root, "src/new.rs"))
            git(root, "switch", "--quiet", "--detach")
            git(root, "config", "--unset", "branch.topic.cairnMode")
            self.assertFalse(self.blocks(root, "src/new.rs"))


if __name__ == "__main__":
    unittest.main()
