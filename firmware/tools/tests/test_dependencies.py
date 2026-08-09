import importlib.util
import json
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


def metadata_root(member_names, dependencies=()):
    """A realistic `cargo metadata --no-deps` root; dependency targets may live in another root."""
    packages = []
    members = []
    for name in member_names:
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
    def test_obc_weather_is_core_and_cannot_pull_in_storage_policy(self):
        production_rules = json.loads((Path(__file__).parents[1] / "dependency_rules.json").read_text())
        self.assertIn("obc-weather", production_rules["groups"]["core"])
        edges = {check_dependencies.Edge("obc-weather", "obc-storage")}
        violations = check_dependencies.check_edges(edges, production_rules)
        self.assertEqual(len(violations), 1)
        self.assertIn("core -> platform", violations[0])

    def test_wx_source_spike_is_host_only(self):
        production_rules = json.loads((Path(__file__).parents[1] / "dependency_rules.json").read_text())
        self.assertEqual(
            check_dependencies.group_index(production_rules)["obc-wx-source-spike"],
            "host",
        )
        edges = {check_dependencies.Edge("obc-weather", "obc-wx-source-spike")}
        violations = check_dependencies.check_edges(edges, production_rules)
        self.assertEqual(len(violations), 1)
        self.assertIn("core -> host", violations[0])

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
        self.assertIn("unclassified production package", violations[0])
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

    def test_excluded_board_target_is_visible_and_rejected(self):
        workspace = metadata_root(
            ("obc-ports",),
            (("obc-ports", "obc-fw-nrf54l", None),),
        )
        board = metadata_root(("obc-fw-nrf54l",))
        packages, edges = check_dependencies.dependency_graph([workspace, board])
        production_rules = {
            "groups": {"foundation": ["obc-ports"], "standalone": ["obc-fw-nrf54l"]},
            "forbidden": [
                {
                    "from_group": "foundation",
                    "to_group": "standalone",
                    "reason": "foundation must stay below composition roots",
                }
            ],
            "exceptions": [],
        }

        self.assertEqual(packages, {"obc-ports", "obc-fw-nrf54l"})
        self.assertIn(check_dependencies.Edge("obc-ports", "obc-fw-nrf54l"), edges)
        violations = check_dependencies.check_edges(edges, production_rules, packages)
        self.assertEqual(len(violations), 1)
        self.assertIn("forbidden dependency edge `obc-ports -> obc-fw-nrf54l`", violations[0])

    def test_excluded_boot_target_is_visible_and_rejected(self):
        workspace = metadata_root(
            ("obc-ports",),
            (("obc-ports", "obc-boot", None),),
        )
        boot = metadata_root(("obc-boot",))
        packages, edges = check_dependencies.dependency_graph([workspace, boot])
        production_rules = {
            "groups": {"foundation": ["obc-ports"], "standalone": ["obc-boot"]},
            "forbidden": [
                {
                    "from_group": "foundation",
                    "to_group": "standalone",
                    "reason": "foundation must stay below composition roots",
                }
            ],
            "exceptions": [],
        }

        self.assertIn(check_dependencies.Edge("obc-ports", "obc-boot"), edges)
        violations = check_dependencies.check_edges(edges, production_rules, packages)
        self.assertEqual(len(violations), 1)
        self.assertIn("forbidden dependency edge `obc-ports -> obc-boot`", violations[0])

    def test_standalone_outgoing_edges_are_in_combined_graph(self):
        workspace = metadata_root(("obc-app",))
        board = metadata_root(
            ("obc-fw-nrf54l",),
            (("obc-fw-nrf54l", "obc-app", None),),
        )
        _, edges = check_dependencies.dependency_graph([workspace, board])
        self.assertIn(check_dependencies.Edge("obc-fw-nrf54l", "obc-app"), edges)

    def test_standalone_manifest_paths_are_relative_to_primary_root(self):
        primary = Path("/repo/Cargo.toml")
        manifests = check_dependencies.metadata_manifests(
            primary,
            {"standalone_manifests": ["firmware/obc-fw-nrf54l/Cargo.toml", "firmware/obc-boot/Cargo.toml"]},
        )
        self.assertEqual(
            manifests,
            [
                primary,
                Path("/repo/firmware/obc-fw-nrf54l/Cargo.toml"),
                Path("/repo/firmware/obc-boot/Cargo.toml"),
            ],
        )


if __name__ == "__main__":
    unittest.main()
