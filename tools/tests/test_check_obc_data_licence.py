import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "check_obc_data_licence.py"
SPEC = importlib.util.spec_from_file_location("check_obc_data_licence", MODULE_PATH)
guard = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = guard
SPEC.loader.exec_module(guard)

REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def metadata(*deps):
    """`obc-data` with `deps`: (name, licence, source, dependency kind)."""
    packages = [{"id": "data", "name": "obc-data", "version": "0.1.0", "license": "MIT OR Apache-2.0", "source": None}]
    edges = []
    for name, licence, source, kind in deps:
        packages.append({"id": name, "name": name, "version": "1.0.0", "license": licence, "source": source})
        edges.append({"pkg": name, "dep_kinds": [{"kind": kind}]})
    nodes = [{"id": "data", "deps": edges}] + [{"id": name, "deps": []} for name, *_ in deps]
    return {"packages": packages, "resolve": {"nodes": nodes}}


class LicenceGuardTests(unittest.TestCase):
    def test_a_gpl_term_joined_by_and_is_copyleft(self):
        self.assertTrue(guard.copyleft("(MIT OR Apache-2.0) AND GPL-3.0-only"))
        self.assertTrue(guard.copyleft("GPL-2.0-only WITH Classpath-exception-2.0"))
        self.assertFalse(guard.copyleft("MIT/Apache-2.0"))
        self.assertFalse(guard.copyleft("(MIT AND GPL-3.0-only) OR Apache-2.0"))

    def test_a_transitive_gpl_dependency_fails(self):
        document = metadata(("glue", "MIT", REGISTRY, None))
        document["packages"].append({"id": "readline", "name": "readline", "version": "1.0.0",
                                     "license": "GPL-3.0-only", "source": REGISTRY})
        document["resolve"]["nodes"][1]["deps"] = [{"pkg": "readline", "dep_kinds": [{"kind": None}]}]
        document["resolve"]["nodes"].append({"id": "readline", "deps": []})
        self.assertEqual(guard.violations(document), ["readline 1.0.0 is `GPL-3.0-only`"])

    def test_permissive_and_dual_licensed_dependencies_pass(self):
        found = guard.violations(metadata(("serde", "MIT OR Apache-2.0", REGISTRY, None),
                                          ("either", "MIT OR GPL-2.0", REGISTRY, "build")))
        self.assertEqual(found, [])

    def test_a_gpl_or_unlicensed_dependency_fails(self):
        found = guard.violations(metadata(("readline", "GPL-3.0-or-later", REGISTRY, None),
                                          ("mystery", None, REGISTRY, None)))
        self.assertEqual(len(found), 2, found)

    def test_a_crate_of_this_repository_fails(self):
        found = guard.violations(metadata(("obc-pack", None, None, None)))
        self.assertIn("crate of this repository", found[0])

    def test_a_development_dependency_is_not_linked(self):
        self.assertEqual(guard.violations(metadata(("obc-pack", None, None, "dev"))), [])


if __name__ == "__main__":
    unittest.main()
