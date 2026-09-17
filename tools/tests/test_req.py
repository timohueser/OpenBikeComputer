"""obc req renders a requirement with numbered criteria and the console's coverage rule."""
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
                                      {"id": "b", "statement": "Choice survives a restart.", "evidence": [{"testId": "t2", "rationale": "Confirms on the device."}], "gap": "Add a save/reload test."},
                                      {"id": "c", "statement": "Heading-up renders.", "evidence": [], "gap": ""}]}}
    return {**base, **overrides}


class RenderTest(unittest.TestCase):
    def test_numbers_criteria_and_marks(self):
        text = req.render(requirement(), REVISION, [])
        self.assertEqual(text.splitlines()[0], "SYS-003 · Map · Partial 1/3 · r58")
        self.assertIn("Coverage — approved by timo, 2026-09-16, commit ad2ada724b", text)
        self.assertIn("1 ✓ North-up renders.\n    North-up projection [map::north_up] — Checks the angle.", text)
        self.assertIn("2 ○ Choice survives a restart.\n    Ride check [manual] — Confirms on the device.\n    gap: Add a save/reload test.", text)
        self.assertIn("3 ○ Heading-up renders.\n    no evidence", text)

    def test_evidence_must_name_a_linked_test_of_the_right_kind(self):
        r = requirement()
        r["coverage"]["criteria"] = [{"id": "x", "statement": "Mixed up.", "evidence": [{"testId": "t1", "rationale": "Names an automated test by ID."}], "gap": ""}]
        self.assertEqual(req.state(r), "Partial 0/1")
        self.assertIn("unlinked [t1]", req.render(r, REVISION, []))
        r["coverage"]["criteria"] = []
        self.assertEqual(req.state(r), "Partial 0/0")

    def test_states_labels_and_pending_proposals(self):
        self.assertEqual(req.state(requirement(coverage=None)), "Not assessed")
        unreviewed = requirement(); del unreviewed["coverage"]["review"]
        self.assertEqual(req.state(unreviewed), "Needs review 1/3")
        flagged = req.render(requirement(active=False, todo=True), REVISION, []).splitlines()[0]
        self.assertEqual(flagged, "SYS-003 · Map · Partial 1/3 · definition incomplete · excluded from releases · r58")
        proposal = {"requirementId": "SYS-003", "status": "pending", "author": "agent", "createdAt": "2026-09-17T08:00:00Z", "sourceSha": "0123456789ab",
                    "plan": {"criteria": [{}, {}, {}]}, "stale": "The statement changed."}
        self.assertIn("Pending proposal by agent (2026-09-17, commit 0123456789): 3 criteria; The statement changed.", req.render(requirement(), REVISION, [proposal]))


if __name__ == "__main__":
    unittest.main()
