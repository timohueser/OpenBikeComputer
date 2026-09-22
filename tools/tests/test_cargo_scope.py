import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "cargo_scope.py"
SPEC = importlib.util.spec_from_file_location("cargo_scope", MODULE_PATH)
cargo_scope = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = cargo_scope
SPEC.loader.exec_module(cargo_scope)


def package(name, *kinds):
    return {"name": name, "targets": [{"name": name, "kind": [kind]} for kind in kinds]}


METADATA = {"packages": [package("obc-sim", "bin", "test"), package("obc-crc", "lib")]}


class ScopeTests(unittest.TestCase):
    def test_both_spellings_of_a_package_and_a_manifest_are_read(self):
        arguments = ["-p", "obc-sim", "--package=obc-crc", "--manifest-path", "apps/Cargo.toml"]
        self.assertEqual(cargo_scope.scope(arguments), (["obc-sim", "obc-crc"], "apps/Cargo.toml"))
        self.assertEqual(cargo_scope.scope(["--manifest-path=a/Cargo.toml"]), ([], "a/Cargo.toml"))

    def test_a_test_filter_and_an_unrelated_flag_are_not_a_scope(self):
        self.assertEqual(cargo_scope.scope(["-p", "obc-sim", "dirty", "--no-capture"]), (["obc-sim"], None))

    def test_a_package_glob_selects_the_packages_it_matches(self):
        selected = cargo_scope.selected(METADATA, ["obc-c*"])
        self.assertEqual([entry["name"] for entry in selected], ["obc-crc"])


class LibraryTests(unittest.TestCase):
    def test_a_binary_only_scope_has_no_library_and_a_mixed_one_has(self):
        self.assertFalse(cargo_scope.has_library(cargo_scope.selected(METADATA, ["obc-sim"])))
        self.assertTrue(cargo_scope.has_library(cargo_scope.selected(METADATA, ["obc-sim", "obc-crc"])))

    def test_no_pattern_asks_about_every_package_in_the_metadata(self):
        self.assertTrue(cargo_scope.has_library(cargo_scope.selected(METADATA, [])))


if __name__ == "__main__":
    unittest.main()
