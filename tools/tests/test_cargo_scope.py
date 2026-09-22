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
    return {
        "name": name,
        "id": f"path+file:///w/{name}#0.1.0",
        "targets": [{"name": name, "kind": [kind]} for kind in kinds],
    }


SIM = package("obc-sim", "bin", "test")
CRC = package("obc-crc", "lib")


def workspace(*default):
    return {"packages": [SIM, CRC], "workspace_default_members": [entry["id"] for entry in default]}


def names(packages):
    return [entry["name"] for entry in packages]


class ScopeTests(unittest.TestCase):
    def test_both_spellings_of_every_selector_are_read(self):
        arguments = ["-p", "obc-sim", "--package=obc-crc", "--exclude", "a", "--exclude=b"]
        chosen = cargo_scope.scope(arguments + ["--manifest-path", "apps/Cargo.toml", "--all"])
        self.assertEqual(chosen["packages"], ["obc-sim", "obc-crc"])
        self.assertEqual(chosen["exclude"], ["a", "b"])
        self.assertEqual(chosen["manifest"], "apps/Cargo.toml")
        self.assertTrue(chosen["workspace"])
        self.assertEqual(cargo_scope.scope(["--manifest-path=a/Cargo.toml"])["manifest"], "a/Cargo.toml")

    def test_a_test_filter_and_an_unrelated_flag_are_not_a_scope(self):
        chosen = cargo_scope.scope(["-p", "obc-sim", "dirty", "--no-capture"])
        self.assertEqual(chosen["packages"], ["obc-sim"])
        self.assertFalse(chosen["workspace"])


class SelectionTests(unittest.TestCase):
    def select(self, *arguments, default=(CRC,)):
        return names(cargo_scope.selected(workspace(*default), cargo_scope.scope(list(arguments))))

    def test_a_package_pattern_selects_the_packages_it_matches(self):
        self.assertEqual(self.select("-p", "obc-c*"), ["obc-crc"])

    def test_a_manifest_alone_selects_its_default_members(self):
        self.assertEqual(self.select("--manifest-path", "apps/obc-sim/Cargo.toml", default=(SIM,)), ["obc-sim"])

    def test_the_whole_workspace_is_selected_only_when_it_is_asked_for(self):
        self.assertEqual(self.select("--workspace"), ["obc-sim", "obc-crc"])
        self.assertEqual(self.select("--all", "--exclude", "obc-c*"), ["obc-sim"])


class LibraryTests(unittest.TestCase):
    def library(self, *arguments, default=(CRC,)):
        return cargo_scope.has_library(cargo_scope.selected(workspace(*default), cargo_scope.scope(list(arguments))))

    def test_a_binary_only_scope_has_no_library_and_a_mixed_one_has(self):
        self.assertFalse(self.library("-p", "obc-sim"))
        self.assertTrue(self.library("-p", "obc-sim", "-p", "obc-crc"))

    def test_a_binary_only_member_is_not_rescued_by_a_sibling_library(self):
        self.assertFalse(self.library("--manifest-path", "apps/obc-sim/Cargo.toml", default=(SIM,)))

    def test_excluding_every_library_leaves_a_scope_with_none(self):
        self.assertFalse(self.library("--workspace", "--exclude", "obc-crc"))


if __name__ == "__main__":
    unittest.main()
