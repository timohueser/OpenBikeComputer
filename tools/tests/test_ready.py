"""Tests for `tools/ready.py`: a set of changed paths maps to a plan.

The gates themselves are never executed here, and nothing in this file needs `just` or Cargo.
"""

from __future__ import annotations

import io
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ready
import test_plan


def package(name: str, root: str, product_root: str = ".") -> test_plan.Package:
    return test_plan.Package(
        name, root, f"{root}/Cargo.toml", product_root, frozenset(), ("main",), (), ()
    )


PACKAGES = {
    "obc-app": package("obc-app", "firmware/obc-app"),
    "obc-render": package("obc-render", "firmware/obc-render"),
    "obc-boot": package("obc-boot", "firmware/obc-boot", "firmware/obc-boot"),
}


RENDERING = ("firmware/obc-app/src/**", "firmware/ui-frames.toml", "firmware/obc-app/i18n/*.toml")

EDGE = "changed Rust package obc-app: firmware/obc-app/src/app.rs"
FOUNDATION = f"{test_plan.FOUNDATION_CHANGED} Cargo.toml"


def unit(identifier: str, command: str = "", reason: str = EDGE) -> test_plan.Unit:
    return test_plan.Unit(id=identifier, jobs=["test"], command=command, reasons=[reason])


class ReadyPlanTests(unittest.TestCase):
    def plan(self, *changed: str, suites=()):
        selected = [value if isinstance(value, test_plan.Unit) else unit(value) for value in suites]
        return ready.plan(
            list(changed),
            base="origin/develop",
            packages=PACKAGES,
            selected=selected,
            rendering=RENDERING,
        )

    def running(self, *changed: str, suites=()):
        return [gate.command for gate in self.plan(*changed, suites=suites) if gate.run]

    def skipped(self, *changed: str, suites=()):
        return {gate.command: gate.reason for gate in self.plan(*changed, suites=suites) if not gate.run}

    def test_documentation_change_plans_the_documentation_gates_only(self):
        changed = ("docs/content/riding.md",)
        self.assertEqual(
            self.running(*changed),
            ["python3 docs/build_docs.py --check-links", "obc docs check"],
        )
        skipped = self.skipped(*changed)
        self.assertEqual(skipped["cargo fmt --all"], "no Rust source changed")
        self.assertEqual(skipped["cargo clippy"], "no Cargo package changed")
        self.assertEqual(skipped["obc shot --check"], "no rendering, screen or i18n input changed")
        self.assertEqual(skipped["obc test affected --base origin/develop"], "the plan selects no unit")

    def test_every_gate_prints_a_reason(self):
        for gate in self.plan("docs/content/riding.md"):
            self.assertTrue(gate.reason.strip(), gate.command)

    def test_single_crate_change_lints_that_crate_only(self):
        running = self.running("firmware/obc-app/src/app.rs", suites={"rust.obc-app"})
        self.assertIn("cargo clippy -p obc-app --all-targets -- -D warnings", running)
        self.assertNotIn("cargo clippy -p obc-render --all-targets -- -D warnings", running)
        self.assertIn("cargo fmt --all", running)
        self.assertIn("obc test affected --base origin/develop", running)

    def test_a_standalone_cargo_root_formats_and_lints_through_its_manifest(self):
        running = self.running("firmware/obc-boot/src/main.rs", suites={"rust.obc-boot"})
        self.assertIn("cargo fmt --manifest-path firmware/obc-boot/Cargo.toml", running)
        self.assertIn(
            "cargo clippy --manifest-path firmware/obc-boot/Cargo.toml --all-targets -- -D warnings",
            running,
        )
        self.assertNotIn("cargo fmt --all", running)

    def test_test_policy_and_test_sources_select_the_suites_check(self):
        for path in ("testing/suites.toml", "tools/test_plan.py", "firmware/ui-frames.toml", "tools/tests/test_ready.py"):
            self.assertIn("obc suites check", self.running(path), path)
        self.assertNotIn("obc suites check", self.running("firmware/obc-app/src/app.rs"))

    def test_a_cargo_manifest_selects_the_licence_check(self):
        self.assertIn("tools/licenses/gen-third-party.sh --check", self.running("Cargo.lock"))
        self.assertIn(
            "tools/licenses/gen-third-party.sh --check",
            self.running("firmware/obc-app/Cargo.toml", suites={"rust.obc-app"}),
        )

    def test_the_affected_run_covers_a_gate_instead_of_repeating_it(self):
        suites = {"rust.obc-app", "ci.ui-snapshots", "ci.licenses", "ci.docs", "ci.rust-clippy"}
        skipped = self.skipped("firmware/obc-app/src/app.rs", "docs/content/riding.md", "Cargo.lock", suites=suites)
        self.assertEqual(skipped["obc shot --check"], "obc test affected runs it as ci.ui-snapshots")
        self.assertEqual(
            skipped["python3 docs/build_docs.py --check-links"], "obc test affected runs it as ci.docs"
        )
        self.assertEqual(
            skipped["tools/licenses/gen-third-party.sh --check"], "obc test affected runs it as ci.licenses"
        )
        self.assertEqual(
            skipped["cargo clippy -p obc-app --all-targets -- -D warnings"],
            "obc test affected runs it as ci.rust-clippy",
        )
        # The format gate writes; the suite behind it only checks, so it is never covered.
        self.assertIn("cargo fmt --all", self.running("firmware/obc-app/src/app.rs", suites=suites))

    def test_a_wholesale_selection_keeps_the_free_suites_and_leaves_the_rest_to_ci(self):
        # A foundation input selects the whole graph. That run is CI's, but the suites that
        # build nothing cost a fraction of a second, so they must not disappear with it.
        tools_tests = "mkdir -p .artifacts && rm -rf .artifacts/python && python3 -m unittest discover"
        selected = [
            unit("rust.obc-app", reason=FOUNDATION),
            unit("ci.rust-clippy", "obc check clippy", FOUNDATION),
            unit("ci.dependency-direction", "python3 firmware/tools/check_dependencies.py", FOUNDATION),
            unit("python.repository-tools", tools_tests, FOUNDATION),
            unit("ci.wasm-size", "bash builder/build-wasm-bridges.sh", FOUNDATION),
            unit("ci.licenses", "tools/licenses/gen-third-party.sh --check", FOUNDATION),
            unit("ci.ui-snapshots", "obc shot --check", FOUNDATION),
            test_plan.Unit(
                id="ios.checks",
                jobs=["ios-unit"],
                command="python3 companion-ios/scripts/check.py",
                platforms=("windows",),
                reasons=[FOUNDATION],
            ),
        ]
        gates = self.plan("Cargo.toml", "firmware/obc-app/src/app.rs", suites=selected)
        lines = {gate.command: gate for gate in gates}

        affected = lines["obc test affected --base origin/develop"]
        self.assertFalse(affected.run)
        self.assertIn(FOUNDATION, affected.reason)
        self.assertEqual(
            [gate.command for gate in gates if gate.run],
            ["cargo fmt --all", "python3 firmware/tools/check_dependencies.py", tools_tests],
        )
        self.assertEqual(lines["bash builder/build-wasm-bridges.sh"].reason, "ci.wasm-size is left to CI")
        self.assertEqual(
            lines["python3 companion-ios/scripts/check.py"].reason, "ios.checks runs only on windows"
        )
        # The snapshot sweep stays CI's work: the budget gives it one run, and CI has it.
        self.assertEqual(lines["obc shot --check"].reason, "ci.ui-snapshots is left to CI")
        # The clippy gate does that suite's work locally, so the suite has no second line.
        self.assertNotIn("obc check clippy", lines)
        self.assertEqual(
            lines["cargo clippy -p obc-app --all-targets -- -D warnings"].reason,
            "ci.rust-clippy is left to CI",
        )
        # A selected Cargo package has no command of its own; the affected gate counts it.
        self.assertNotIn("", lines)
        for gate in gates:
            self.assertTrue(gate.reason.strip(), gate.command)

    def test_a_rendering_input_selects_the_sweep_when_no_suite_runs_it(self):
        self.assertIn(
            "obc shot --check", self.running("firmware/obc-app/i18n/de.toml", suites={"rust.obc-app"})
        )

    def test_only_the_format_gate_writes(self):
        writing = [gate.command for gate in self.plan("firmware/obc-app/src/app.rs") if gate.writes]
        self.assertEqual(writing, ["cargo fmt --all"])

    def test_a_rewritten_file_is_reported_instead_of_the_skeleton(self):
        # Two status readings: the tree before the format gate, and the tree it left behind.
        readings = iter(({"src/b.rs": " M"}, {"src/a.rs": " M", "src/b.rs": " M"}))
        gates = [ready.Gate("true", "demo", True, writes=True), ready.Gate("true", "demo", True)]

        with redirect_stdout(io.StringIO()):
            code, formatted = ready.run_gates(gates, Path("."), status=lambda _root: next(readings))

        self.assertEqual((code, formatted), (0, ["src/a.rs"]))

    def test_an_unchanged_tree_reports_nothing(self):
        gates = [ready.Gate("true", "demo", True, writes=True)]
        with redirect_stdout(io.StringIO()):
            code, formatted = ready.run_gates(gates, Path("."), status=lambda _root: {"src/b.rs": " M"})
        self.assertEqual((code, formatted), (0, []))

    def test_a_human_page_that_cites_a_changed_source_is_reported(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            content = root / "docs/content"
            content.mkdir(parents=True)
            front = "---\ntitle: Test\ndescription: Test page.\ncopy: %s\n---\n\n# Test\n\n"
            (content / "human.md").write_text(front % "human" + "See [src:firmware/x.rs].", encoding="utf-8")
            (content / "draft.md").write_text(front % "ai" + "See [src:firmware/x.rs].", encoding="utf-8")

            self.assertEqual(ready.human_pages(root, ["firmware/x.rs"]), ["docs/content/human.md"])
            self.assertEqual(ready.human_pages(root, ["firmware/other.rs"]), [])

    def test_surfaces_name_the_changed_areas(self):
        self.assertEqual(
            ready.surfaces(["firmware/obc-app/src/app.rs", "docs/content/riding.md", "Cargo.lock"]),
            ["(repository root)", "docs", "firmware"],
        )


if __name__ == "__main__":
    unittest.main()
