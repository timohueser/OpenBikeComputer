import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "check_dependencies.py"
SPEC = importlib.util.spec_from_file_location("check_dependencies", SCRIPT)
check_dependencies = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = check_dependencies
SPEC.loader.exec_module(check_dependencies)


def metadata(*dependencies):
    packages = []
    members = []
    names = {"low", "high", *(target for _, target, _ in dependencies)}
    for name in names:
        package_id = f"path+file:///{name}#0.1.0"
        members.append(package_id)
        packages.append(
            {
                "id": package_id,
                "name": name,
                "dependencies": [
                    {"name": target, "kind": kind}
                    for source, target, kind in dependencies
                    if source == name
                ],
            }
        )
    return {"workspace_members": members, "packages": packages}


def rules(exceptions=()):
    return {
        "groups": {"low": ["low"], "high": ["high"]},
        "forbidden": [
            {"from_group": "low", "to_group": "high", "reason": "low must stay low"}
        ],
        "exceptions": list(exceptions),
    }


class DependencyTests(unittest.TestCase):
    def test_forbidden_edge_has_useful_message(self):
        edges = check_dependencies.local_edges(metadata(("low", "high", None)))
        violations = check_dependencies.check_edges(edges, rules())
        self.assertEqual(len(violations), 1)
        self.assertIn("forbidden dependency edge `low -> high`", violations[0])
        self.assertIn("low must stay low", violations[0])

    def test_dev_edges_do_not_constrain_test_fixtures(self):
        edges = check_dependencies.local_edges(metadata(("low", "high", "dev")))
        self.assertEqual(edges, set())

    def test_named_exception_allows_existing_debt(self):
        exception = {"from": "low", "to": "high", "issue": "#1", "reason": "migration"}
        edges = check_dependencies.local_edges(metadata(("low", "high", None)))
        self.assertEqual(check_dependencies.check_edges(edges, rules((exception,))), [])

    def test_stale_exception_forces_allowlist_tightening(self):
        exception = {"from": "low", "to": "high", "issue": "#1", "reason": "migration"}
        violations = check_dependencies.check_edges(set(), rules((exception,)))
        self.assertEqual(len(violations), 1)
        self.assertIn("stale dependency exception", violations[0])

    def test_ambiguous_group_membership_is_rejected(self):
        with self.assertRaisesRegex(check_dependencies.DependencyError, "ambiguous"):
            check_dependencies.group_index({"groups": {"one": ["same"], "two": ["same"]}})

    def test_unclassified_workspace_package_cannot_evade_rules(self):
        violations = check_dependencies.check_edges(set(), rules(), {"low", "high", "new-crate"})
        self.assertEqual(len(violations), 1)
        self.assertIn("unclassified production workspace package", violations[0])
        self.assertIn("`new-crate`", violations[0])

    def test_unknown_forbidden_group_is_rejected(self):
        invalid = rules()
        invalid["forbidden"][0]["from_group"] = "typo-low"
        with self.assertRaisesRegex(check_dependencies.DependencyError, "unknown from_group `typo-low`"):
            check_dependencies.check_edges(set(), invalid)

    def test_duplicate_forbidden_pair_is_rejected(self):
        invalid = rules()
        invalid["forbidden"].append(dict(invalid["forbidden"][0]))
        with self.assertRaisesRegex(check_dependencies.DependencyError, "duplicate forbidden dependency pair"):
            check_dependencies.check_edges(set(), invalid)

    def test_duplicate_exception_edge_is_rejected(self):
        exception = {"from": "low", "to": "high", "issue": "#1", "reason": "migration"}
        invalid = rules((exception, dict(exception)))
        with self.assertRaisesRegex(check_dependencies.DependencyError, "duplicate dependency exception"):
            check_dependencies.check_edges({check_dependencies.Edge("low", "high")}, invalid)


if __name__ == "__main__":
    unittest.main()
