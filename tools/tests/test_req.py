"""obc req: the rendering, the listing, the revision delta, and the checks that gate a submission."""
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
        self.assertIn("1 ✓ North-up renders.\n    map::north_up   (not in the catalogue)\n        Checks the angle.", text)
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
        return req.plan_problems(entry, requirement(), baseline, self.CATALOG)

    def test_a_clean_plan_has_nothing_to_say(self):
        self.assertEqual(self.check(self.entry()), [])

    def test_a_gap_must_name_the_test_to_build_and_a_named_test_must_have_a_gap(self):
        gapped = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": "Missing."}])
        self.assertIn("does not name the test to build", " ".join(self.check(gapped)))
        loose = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": "",
                                      "next": {"level": "unit", "summary": "y"}}])
        self.assertIn("records no gap", " ".join(self.check(loose)))

    def test_levels_and_summaries_are_bounded(self):
        bad = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": "g",
                                    "next": {"level": "smoke", "summary": "y"}}])
        self.assertIn("use one of unit, integration, system, ride", " ".join(self.check(bad)))
        long = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [], "gap": "g",
                                     "next": {"level": "unit", "summary": "y" * 301}}])
        self.assertIn("keep it to one sentence", " ".join(self.check(long)))

    def test_evidence_must_resolve_and_name_exactly_one_test(self):
        unknown = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [{"caseId": "nope", "rationale": "r"}], "gap": ""}])
        self.assertIn("not in the catalogue", " ".join(self.check(unknown)))
        both = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [{"caseId": "map::north_up", "testId": "t2", "rationale": "r"}], "gap": ""}])
        self.assertIn("neither exactly one case nor one manual test", " ".join(self.check(both)))
        missing = self.entry(criteria=[{"id": "a", "statement": "x", "evidence": [{"testId": "ghost", "rationale": "r"}], "gap": ""}])
        self.assertIn("does not exist", " ".join(self.check(missing)))

    def test_criterion_ids_may_not_drift_from_the_plan_being_revised(self):
        baseline = {"criteria": [{"id": "a"}, {"id": "b"}]}
        renamed = self.entry(criteria=[{"id": "z", "statement": "x", "evidence": [], "gap": ""}])
        problems = " ".join(self.check(renamed, baseline))
        self.assertIn("criterion a is in the plan being revised", problems)
        self.assertIn("criterion z is new", problems)

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


if __name__ == "__main__":
    unittest.main()
