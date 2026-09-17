"""obc req renders a requirement with numbered criteria and an honest coverage state."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import req  # noqa: E402

REVISION = {"id": 58}
CATALOG = {"cases": [{"id": "map::north_up", "name": "north up"}, {"id": "map::restart", "name": "restart"}]}


def requirement(**overrides):
    base = {"id": "SYS-003", "group": "Map", "title": "Map orientation", "statement": "The user shall choose the orientation.", "active": True,
            "tests": [{"id": "t1", "kind": "automated", "title": "North-up projection", "caseId": "map::north_up"}],
            "coverage": {"rationale": "Render tests cover both modes.", "review": {"author": "timo", "createdAt": "2026-09-16T10:00:00Z", "sourceSha": "ad2ada724b00"},
                         "criteria": [{"id": "a", "statement": "North-up renders.", "evidence": [{"caseId": "map::north_up", "rationale": "Checks the angle."}], "gap": ""},
                                      {"id": "b", "statement": "Choice survives a restart.", "evidence": [], "gap": "Add a save/reload test."}]}}
    return {**base, **overrides}


def test_render_numbers_criteria_and_states():
    text = req.render(requirement(), REVISION, CATALOG, [])
    assert text.splitlines()[0] == "SYS-003 · Map · Partial 1/2 · r58"
    assert "Coverage — approved by timo, 2026-09-16, commit ad2ada724b" in text
    assert "1 ✓ North-up renders." in text and "    North-up projection [map::north_up] — Checks the angle." in text
    assert "2 ○ Choice survives a restart." in text and "    no evidence" in text and "    gap: Add a save/reload test." in text


def test_render_states_without_plan_or_review_and_pending_proposal():
    assert req.state(requirement(coverage=None)) == "Not assessed"
    unreviewed = requirement(); del unreviewed["coverage"]["review"]
    assert req.state(unreviewed) == "Needs review 1/2"
    proposal = {"requirementId": "SYS-003", "status": "pending", "author": "agent", "createdAt": "2026-09-17T08:00:00Z", "sourceSha": "0123456789ab", "plan": {"criteria": [{}, {}, {}]}}
    assert "Pending proposal by agent (2026-09-17, commit 0123456789): 3 criteria" in req.render(requirement(), REVISION, CATALOG, [proposal])
