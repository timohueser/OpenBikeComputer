"""Differential harness for the selector cutover. Deleted in the same pull request.

It runs the registry that `tools/test_plan.py` replaces and the new planner over the same
inputs and requires the same answer. The old script is not in the tree any more, so this
reads four pre-cutover files the cutover keeps under `.scratch/`, taken from develop:
`tools/suite_registry.py`, `testing/suites.toml`, `.github/workflows/ci.yml` and
`tools/ci/test.sh`, named `ts-c2-old-registry.py`, `ts-c2-old-suites.toml`,
`ts-c2-old-ci.yml` and `ts-c2-old-test.sh`. Then:

    python3 -m unittest discover -s tools/tests -p test_plan_diff.py -v

Inputs: the fourteen shipped change classes, and the last thirty merge commits on develop
with each merge's first parent as the base.

Required jobs, selection errors and selected unit ids must agree, except for the two
differences the cutover makes on purpose:

  E1  The five `fixtures.rust-*` suite rows are retired. `required-features =
      ["external-fixtures"]` is a Cargo fact, so the captured-fixture tier is derived and
      the package's own unit (`rust.obc-route`, …) carries the selection instead.
  E2  `rust.obc-bench` is no longer selected by a test-policy change. The old registry
      gave it a `workflow` ownership row for the bench golden command, which made every
      policy path select it. Its jobs stay required through `ci.rust-clippy`,
      `ci.rust-format` and `ci.rust-workspace-tests`.
"""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import test_plan as new

ROOT = new.repository_root()
OLD_REGISTRY = ROOT / ".scratch/ts-c2-old-registry.py"
OLD_SUITES = ROOT / ".scratch/ts-c2-old-suites.toml"
OLD_WORKFLOW = ROOT / ".scratch/ts-c2-old-ci.yml"
OLD_SCRIPT = ROOT / ".scratch/ts-c2-old-test.sh"

E1_RETIRED = {f"fixtures.obc-{name}" for name in ("dem", "host-core", "reader", "route", "sim")}
E1_REPLACEMENT = {f"rust.obc-{name}" for name in ("dem", "host-core", "reader", "route", "sim")}
E2_BENCH = {"rust.obc-bench"}

CHANGE_CLASSES = [
    ("documentation only", ["docs/content/ride.md"]),
    ("leaf Rust crate", ["host/obc-bench/src/main.rs"]),
    ("foundational Rust crate", ["firmware/obc-crc/src/lib.rs"]),
    ("shared vectors", ["specs/vectors/obcm-v2.json"]),
    ("desktop launch harness", ["apps/obc-desktop/e2e/launch.py"]),
    ("iOS application", ["companion-ios/OBCCompanion/App.swift"]),
    ("web only", ["builder/app/src/lib/panel.ts"]),
    ("workflow", [".github/workflows/ci.yml"]),
    ("nextest configuration", [".config/nextest.toml"]),
    ("web demo crate", ["apps/obc-web-demo/src/lib.rs"]),
    ("web demo Trunk target", ["docs/index.html"]),
    ("web demo browser harness", ["apps/obc-web-demo/tests/browser/ride-log.test.js"]),
    ("OBCKit package source", ["companion-ios/Packages/OBCKit/Sources/OBCTransport/BLE/Client.swift"]),
    ("repository tooling", ["tools/fixtures.py"]),
]


def load_old(base: Path):
    """The registry, reading the pre-cutover registry file, workflow and job script."""

    (base / ".github/workflows").mkdir(parents=True)
    (base / ".github/workflows/ci.yml").write_bytes(OLD_WORKFLOW.read_bytes())
    (base / "tools/ci").mkdir(parents=True)
    (base / "tools/ci/test.sh").write_bytes(OLD_SCRIPT.read_bytes())

    spec = importlib.util.spec_from_file_location("old_registry", OLD_REGISTRY)
    module = importlib.util.module_from_spec(spec)
    sys.modules["old_registry"] = module
    spec.loader.exec_module(module)
    read_toml = module._read_toml
    module._read_toml = lambda path: read_toml(OLD_SUITES if path.name == "suites.toml" else path)
    for name in ("scan_workflow", "workflow_jobs", "aggregate_job"):
        original = getattr(module, name)
        setattr(module, name, lambda _root=None, _call=original: _call(base))
    return module


@unittest.skipUnless(
    all(path.is_file() for path in (OLD_REGISTRY, OLD_SUITES, OLD_WORKFLOW, OLD_SCRIPT)),
    "the pre-cutover selector is not staged",
)
class DifferentialTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.base = tempfile.TemporaryDirectory()
        cls.old = load_old(Path(cls.base.name))
        cls.inventory = cls.old.load_inventory(ROOT)
        cls.old_graph = cls.old.build_cargo_graph(ROOT)
        cls.routes = cls.old.suite_workflow_jobs(cls.inventory, ROOT, cls.old_graph)
        cls.old_jobs = cls.old.workflow_jobs(ROOT)
        cls.unconditional = cls.old.unconditional_jobs(ROOT)
        cls.graph, _, cls.units = new.load(ROOT)

    @classmethod
    def tearDownClass(cls) -> None:
        cls.base.cleanup()

    def compare(self, paths, deleted=()):
        before = self.old.selection_plan_data(
            self.old.select_suites(
                self.inventory, list(paths), self.old_graph, self.routes, unconditional=self.unconditional
            ),
            self.old_jobs,
        )
        after = new.plan_data(new.select(self.units, self.graph, list(paths), deleted=deleted))
        self.assertEqual(before["required_jobs"], after["required_jobs"], "required jobs")
        self.assertEqual(before["errors"], after["errors"], "selection errors")
        old_units = set(before["selected_suite_ids"])
        new_units = set(after["selected_suite_ids"])
        self.assertLessEqual(old_units - new_units, E1_RETIRED | E2_BENCH, "unexplained loss")
        self.assertLessEqual(new_units - old_units, E1_REPLACEMENT, "unexplained gain")

    def test_shipped_change_classes(self) -> None:
        for name, paths in CHANGE_CLASSES:
            with self.subTest(change=name):
                self.compare(paths)

    def test_last_thirty_merges(self) -> None:
        log = subprocess.run(
            ["git", "log", "--merges", "-30", "--format=%H %P"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout
        merges = [line.split() for line in log.splitlines() if line.strip()]
        self.assertEqual(len(merges), 30)
        for head, base, *_ in merges:
            with self.subTest(merge=head[:9]):
                paths, deleted = new.git_changed_paths(ROOT, base, head)
                self.compare(paths, deleted)


if __name__ == "__main__":
    unittest.main()
