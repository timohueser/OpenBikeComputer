"""obc req: the rendering, the listing, the revision delta, and the checks that gate a submission."""
import contextlib
import io
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import req  # noqa: E402

REVISION = {"id": 58}
NORTH = {"id": "t1", "kind": "automated", "title": "North-up projection", "caseId": "map::north_up"}
RIDE = {"id": "t2", "kind": "manual", "title": "Ride check", "steps": "Ride.", "expected": "Stays."}


def requirement(**overrides):
    base = {"id": "SYS-003", "group": "Map", "title": "Map orientation", "statement": "The user shall choose the orientation.", "active": True,
            "tests": [NORTH, RIDE],
            "coverage": {"rationale": "Render tests cover both modes.", "review": {"author": "timo", "createdAt": "2026-09-16T10:00:00Z", "sourceSha": "ad2ada724b00"},
                         "criteria": [{"id": "a", "statement": "North-up renders.", "evidence": [{"caseId": "map::north_up", "rationale": "Checks the angle."}], "gap": ""},
                                      {"id": "b", "statement": "Choice survives a restart.", "evidence": [{"testId": "t2", "rationale": "Confirms on the device."}], "gap": "Add a save/reload test.", "next": {"level": "unit", "summary": "Reload the setting and assert it."}},
                                      {"id": "c", "statement": "Heading-up renders.", "evidence": [], "gap": ""}]}}
    return {**base, **overrides}


class RenderTest(unittest.TestCase):
    def test_numbers_criteria_and_marks(self):
        text = req.render(requirement(), REVISION, [])
        self.assertEqual(text.splitlines()[0], "SYS-003 · Map · Partial 1/3 · r58")
        self.assertIn("Coverage — approved by timo, 2026-09-16, commit ad2ada724b", text)
        self.assertIn("1 ✓ North-up renders.\n    map::north_up\n        Checks the angle.", text)
        self.assertIn("(not in the catalogue)", "\n".join(req.criteria_lines(requirement(), requirement()["coverage"], known=set())))
        self.assertIn("2 ○ Choice survives a restart.\n    manual: Ride check\n        Confirms on the device.\n    gap: Add a save/reload test.\n    next [unit]: Reload the setting and assert it.", text)
        self.assertIn("3 ○ Heading-up renders.\n    no evidence", text)

    def test_evidence_must_name_a_linked_test_of_the_right_kind(self):
        r = requirement()
        r["coverage"]["criteria"] = [{"id": "x", "statement": "Mixed up.", "evidence": [{"testId": "t1", "rationale": "Names an automated test by ID."}], "gap": ""}]
        self.assertEqual(req.state(r), "Partial 0/1")
        self.assertIn("unlinked manual test t1", req.render(r, REVISION, []))
        r["coverage"]["criteria"] = []
        self.assertEqual(req.state(r), "Partial 0/0")

    def test_a_pending_proposal_is_named_even_with_no_approved_plan(self):
        proposal = {"requirementId": "SYS-003", "status": "pending", "author": "agent", "createdAt": "2026-09-17T08:00:00Z",
                    "sourceSha": "0123456789ab", "plan": {"criteria": [{}, {}]}}
        text = req.render(requirement(coverage=None), REVISION, [proposal])
        self.assertIn("No approved coverage plan.", text)
        self.assertIn("Pending proposal by agent (2026-09-17, commit 0123456789): 2 criteria", text)
        self.assertIn("1 criterion;", req.render(requirement(coverage=None), REVISION, [{**proposal, "plan": {"criteria": [{}]}}]))

    def test_states_labels_and_pending_proposals(self):
        self.assertEqual(req.state(requirement(coverage=None)), "Not assessed")
        unreviewed = requirement(); del unreviewed["coverage"]["review"]
        self.assertEqual(req.state(unreviewed), "Needs review 1/3")
        flagged = req.render(requirement(active=False, todo=True), REVISION, []).splitlines()[0]
        self.assertEqual(flagged, "SYS-003 · Map · Partial 1/3 · definition incomplete · excluded from releases · r58")
        proposal = {"requirementId": "SYS-003", "status": "pending", "author": "agent", "createdAt": "2026-09-17T08:00:00Z", "sourceSha": "0123456789ab",
                    "plan": {"criteria": [{}, {}, {}]}, "stale": "The statement changed."}
        self.assertIn("Pending proposal by agent (2026-09-17, commit 0123456789): 3 criteria; The statement changed.", req.render(requirement(), REVISION, [proposal]))


class ProposalTest(unittest.TestCase):
    PROPOSAL = {"requirementId": "SYS-003", "status": "pending", "author": "agent", "baseRevision": 57,
                "createdAt": "2026-09-17T08:00:00Z", "sourceSha": "0123456789ab",
                "plan": {"rationale": "Two modes render.", "criteria": [
                    {"id": "a", "statement": "North-up renders.", "evidence": [{"caseId": "map::north_up", "rationale": "Checks the angle."}], "gap": ""},
                    {"id": "d", "statement": "It holds on a ride.", "evidence": [{"testId": "ride-check", "rationale": "Confirms on the device."}],
                     "gap": "No automated check.", "next": {"level": "ride", "summary": "Ride it."}}]},
                "procedures": [{"id": "ride-check", "title": "Ride check", "steps": "1. Ride.", "expected": "It holds."}]}

    def test_a_proposals_own_new_procedure_resolves_as_evidence(self):
        text = req.render_proposal(self.PROPOSAL, requirement())
        self.assertIn("against r57", text)
        self.assertIn("2 ○ It holds on a ride.\n    manual: Ride check\n        Confirms on the device.", text)
        self.assertIn("New manual procedure [ride-check]: Ride check", text)
        self.assertIn("CONFLICT: gone", req.render_proposal({**self.PROPOSAL, "conflict": "gone"}, requirement()))


class ValidationTest(unittest.TestCase):
    CATALOG = {"map::north_up"}

    def entry(self, **plan_overrides):
        plan = {"rationale": "Fine.", "criteria": [
            {"id": "a", "statement": "North-up renders.", "evidence": [{"caseId": "map::north_up", "rationale": "Checks."}], "gap": ""}]}
        return {"requirementId": "SYS-003", "plan": {**plan, **plan_overrides}}

    def check(self, entry, baseline=None):
        """Only what blocks a submission; the warnings are asserted on their own below."""
        return req.plan_problems(entry, requirement(), baseline, self.CATALOG)[0]

    def notes(self, entry, baseline=None):
        return req.plan_problems(entry, requirement(), baseline, self.CATALOG)[1]

    def test_a_clean_plan_has_nothing_to_say(self):
        self.assertEqual(self.check(self.entry()), [])

    def test_a_missing_next_and_a_new_criterion_are_said_but_do_not_block(self):
        """Adding a criterion is the usual reason to revise a plan, and the console's own demo
        proposal does exactly that, so neither of these may stop a submission."""
        gapped = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": "Missing."}])
        self.assertEqual(self.check(gapped), [])
        self.assertIn("names no test to build", " ".join(self.notes(gapped)))
        added = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": ""},
                                     {"id": "zoom", "statement": "y", "evidence": [], "gap": "New obligation.",
                                      "next": {"level": "unit", "summary": "z"}}])
        self.assertEqual(self.check(added, {"criteria": [{"id": "a"}]}), [])
        self.assertIn("criterion zoom is new", " ".join(self.notes(added, {"criteria": [{"id": "a"}]})))

    def test_levels_and_summaries_are_bounded(self):
        bad = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": "g",
                                    "next": {"level": "smoke", "summary": "y"}}])
        self.assertIn("use one of unit, integration, system, ride", " ".join(self.check(bad)))
        long = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": "g",
                                     "next": {"level": "unit", "summary": "y" * 301}}])
        self.assertIn("keep it to one sentence", " ".join(self.check(long)))

    def test_evidence_must_resolve_and_name_exactly_one_test(self):
        unknown = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [{"caseId": "nope", "rationale": "r"}], "gap": ""}])
        self.assertIn("neither in the catalogue nor already linked", " ".join(self.check(unknown)))
        both = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [{"caseId": "map::north_up", "testId": "t2", "rationale": "r"}], "gap": ""}])
        self.assertIn("neither exactly one case nor one manual test", " ".join(self.check(both)))
        missing = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [{"testId": "ghost", "rationale": "r"}], "gap": ""}])
        self.assertIn("does not exist", " ".join(self.check(missing)))

    def test_the_servers_own_rules_are_all_refusals(self):
        blank = self.entry(criteria=[{"id": "a", "statement": "  ", "evidence": [], "gap": ""}])
        self.assertIn("has no statement", " ".join(self.check(blank)))
        twice = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": ""},
                                     {"id": "a", "statement": "y", "evidence": [], "gap": ""}])
        self.assertIn("criterion id a is used more than once", " ".join(self.check(twice)))
        dupe = self.entry(criteria=[{"id": "a", "statement": "x", "gap": "", "evidence": [
            {"caseId": "map::north_up", "rationale": "r"}, {"caseId": "map::north_up", "rationale": "r"}]}])
        self.assertIn("cites map::north_up twice", " ".join(self.check(dupe)))
        odd = self.entry(criteria=[{"id": "a b", "statement": "x", "evidence": [], "gap": ""}])
        self.assertIn("is not one the server accepts", " ".join(self.check(odd)))
        empty = self.entry(criteria=[{"id": "a", "statement": "x", "gap": "", "evidence": [
            {"caseId": "", "testId": "t2", "rationale": "r"}]}])
        self.assertIn("neither exactly one case nor one manual test", " ".join(self.check(empty)))
        long_id = self.entry(criteria=[{"id": "a" * 201, "statement": "x", "evidence": [], "gap": ""}])
        self.assertIn("is not one the server accepts", " ".join(self.check(long_id)))

    def test_a_null_or_absent_field_is_named_rather_than_raising(self):
        """A generator that writes null for "nothing here" must get a sentence, not a traceback."""
        for criterion, expected in (
            ({"id": "a", "statement": "x", "evidence": [], "gap": None}, "needs a gap string"),
            ({"id": "a", "statement": "x", "evidence": []}, "needs a gap string"),
            ({"id": "a", "statement": "x", "gap": "", "evidence": None}, "needs an evidence list"),
            ({"id": "a", "statement": "x", "gap": ""}, "needs an evidence list"),
            ({"id": "a", "statement": None, "evidence": [], "gap": ""}, "has no statement"),
            ({"id": "a", "statement": "x", "evidence": [], "gap": "g", "next": "soon"}, "next that is not an object"),
        ):
            self.assertIn(expected, " ".join(self.check(self.entry(criteria=[criterion]))))
        self.assertIn("the plan has no rationale", " ".join(self.check(self.entry(rationale=None))))

    def test_a_long_rationale_is_a_note_because_the_server_allows_it(self):
        entry = self.entry(rationale="y" * 901)
        self.assertEqual(self.check(entry), [])
        self.assertIn("reads long on the card", " ".join(self.notes(entry)))

    def test_a_case_the_requirement_already_links_stays_valid_evidence(self):
        """The server accepts evidence naming a case the requirement links, catalogue or not."""
        entry = self.entry(criteria=[{"id": "a", "statement": "x", "gap": "", "evidence": [
            {"caseId": "map::north_up", "rationale": "r"}]}])
        self.assertEqual(req.plan_problems(entry, requirement(), None, set())[0], [])

    def test_a_procedure_must_be_cited_and_complete(self):
        entry = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": "g",
                                      "next": {"level": "ride", "summary": "y"}}])
        entry["procedures"] = [{"id": "spare", "title": "T", "steps": "S", "expected": ""}]
        problems = " ".join(self.check(entry))
        self.assertIn("cited by no criterion", problems)
        self.assertIn("has no expected", problems)
        entry["procedures"] = [{"id": "t2", "title": "T", "steps": "S", "expected": "E"}]
        entry["plan"]["criteria"][0]["evidence"] = [{"testId": "t2", "rationale": "r"}]
        self.assertIn("collides with a test the requirement already has", " ".join(self.check(entry)))


class ListAndDeltaTest(unittest.TestCase):
    def revision(self, *requirements, id=58):
        return {"id": id, "requirements": list(requirements)}

    def test_filters_pick_the_requirement_out(self):
        covered = requirement(id="SYS-001", title="Covered one")
        covered["coverage"]["criteria"] = [covered["coverage"]["criteria"][0]]
        revision = self.revision(covered, requirement(id="SYS-002", coverage=None))
        proposal = {"requirementId": "SYS-002", "status": "pending", "stale": "changed"}
        self.assertEqual(len(req.list_lines(revision, [], set())), 2)
        self.assertIn("SYS-002", req.list_lines(revision, [], {"no-plan"})[0])
        self.assertIn("SYS-003", req.list_lines(self.revision(requirement()), [], {"gaps"})[0])
        self.assertEqual(req.list_lines(revision, [], {"gaps"}), [])
        self.assertIn("flagged proposal", req.list_lines(revision, [proposal], {"flagged"})[0])

    def test_the_delta_names_what_an_agent_has_to_act_on(self):
        before = self.revision(requirement(), id=57)
        after = self.revision(requirement(statement="The rider shall choose the orientation.", implementationNeeded=True),
                              requirement(id="SYS-009", title="Seams"), id=58)
        text = "\n".join(req.changed_lines(before, after))
        self.assertIn("r57 → r58", text)
        self.assertIn("ADDED SYS-009 · Seams", text)
        self.assertIn("CHANGED SYS-003 · Map orientation (statement, implementationNeeded)", text)
        self.assertIn("  was: The user shall choose the orientation.", text)
        self.assertIn("  implementationNeeded: False → True", text)
        self.assertIn("REMOVED SYS-003", "\n".join(req.changed_lines(before, self.revision(id=58))))
        self.assertIn("No requirement changed.", "\n".join(req.changed_lines(before, self.revision(requirement(), id=58))))

    def test_a_flag_reads_either_spelling(self):
        self.assertEqual(req.flag_value(["changed", "--since", "16"], "since"), "16")
        self.assertEqual(req.flag_value(["changed", "--since=16"], "since"), "16")
        self.assertIsNone(req.flag_value(["changed"], "since"))


class WritePathTest(unittest.TestCase):
    """The only command that writes. Its refusals matter more than its output."""

    class Stub:
        def __init__(self, outer):
            self.outer, self.posts = outer, []

        def call(self, path, body=None):
            if body is not None:
                self.posts.append(body)
                return {"id": "new"}
            if path == "/api/bootstrap":
                return {"revision": {"id": 58, "requirements": [requirement()]}}
            if path == "/api/coverage-proposals":
                return []
            return {"cases": [{"id": "map::north_up", "name": "North-up", "suite": "s"}], "sourceSha": "a" * 40}

    def setUp(self):
        self.stub = self.Stub(self)
        self.original, req.Console = req.Console, lambda: self.stub
        self.addCleanup(lambda: setattr(req, "Console", self.original))
        self.plan = Path(__file__).with_name("_plan.json")
        self.plan.write_text(json.dumps({"requirementId": "SYS-003", "plan": {
            "rationale": "Fine.", "criteria": [{"id": "a", "statement": "x", "gap": "",
                                                "evidence": [{"caseId": "map::north_up", "rationale": "r"}]}]}}))
        self.addCleanup(self.plan.unlink)

    def run_cli(self, *argv):
        with contextlib.redirect_stdout(io.StringIO()):
            return req.main(list(argv))

    def test_check_submits_nothing_and_a_mistyped_flag_refuses(self):
        self.assertEqual(self.run_cli("propose", str(self.plan), "--check", f"--sha={'a' * 40}"), 0)
        self.assertEqual(self.stub.posts, [])
        with self.assertRaises(req.Problem) as caught:
            self.run_cli("propose", str(self.plan), "--checks", f"--sha={'a' * 40}")
        self.assertIn("does not take --checks", str(caught.exception))
        self.assertEqual(self.stub.posts, [], "a typo in the safety flag must never submit")

    def test_a_short_sha_is_refused_before_anything_is_sent(self):
        with self.assertRaises(req.Problem) as caught:
            self.run_cli("propose", str(self.plan), "--sha=deadbeef")
        self.assertIn("exact 40-character commit", str(caught.exception))
        self.assertEqual(self.stub.posts, [])

    def test_one_bad_file_stops_the_whole_batch(self):
        bad = self.plan.with_name("_bad.json")
        bad.write_text(json.dumps({"requirementId": "SYS-003", "plan": {"rationale": "", "criteria": []}}))
        self.addCleanup(bad.unlink)
        self.assertEqual(self.run_cli("propose", str(self.plan), str(bad), f"--sha={'a' * 40}"), 1)
        self.assertEqual(self.stub.posts, [])

    def test_a_plan_file_that_will_not_load_is_a_sentence(self):
        broken = self.plan.with_name("_broken.json")
        broken.write_text("{nope")
        self.addCleanup(broken.unlink)
        for argv, expected in (((str(broken),), "not valid JSON"),
                               ((str(self.plan.with_name("_missing.json")),), "could not read")):
            with self.assertRaises(req.Problem) as caught:
                self.run_cli("propose", *argv, f"--sha={'a' * 40}")
            self.assertIn(expected, str(caught.exception))

    def test_unknown_commands_and_stray_arguments_refuse(self):
        for argv, expected in ((("bogus",), "unknown command"),
                               (("list", "--gap"), "does not take --gap"),
                               (("changed", "16"), "did you mean --since 16"),
                               (("tests", "--json"), "does not take --json")):
            with self.assertRaises(req.Problem) as caught:
                self.run_cli(*argv)
            self.assertIn(expected, str(caught.exception))


if __name__ == "__main__":
    unittest.main()
