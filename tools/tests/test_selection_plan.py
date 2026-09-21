"""Tests for `tools/test_plan.py`.

The file is not called `test_plan.py`: unittest discovery from `tools/tests` would then
import the test module under the name the planner already uses, and the planner's own
`import test_plan` would find the tests instead.
"""

from __future__ import annotations

import re
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import test_plan as plan


def package(name, root, dependencies=(), product_root=".", ordinary=("main",), fixtures=(), examples=()):
    return plan.Package(
        name, root, f"{root}/Cargo.toml", product_root, frozenset(dependencies), ordinary, fixtures, examples
    )


def unit(unit_id, jobs, **overrides):
    return plan.Unit(id=unit_id, jobs=list(jobs), declared=True, command=f"run {unit_id}", **overrides)


class SelectionTests(unittest.TestCase):
    """Path, Cargo-graph and fail-closed behaviour on a synthetic repository."""

    def setUp(self) -> None:
        self.graph = plan.CargoGraph(
            {
                "core": package("core", "crates/core", fixtures=("captured",)),
                "leaf": package("leaf", "crates/leaf", ["core"]),
                "consumer": package("consumer", "crates/consumer", ["leaf"]),
                "board": package("board", "firmware/board", ["core"], product_root="firmware/board"),
                "bridge": package("bridge", "crates/bridge"),
            },
            {
                "core": frozenset({"leaf", "board"}),
                "leaf": frozenset({"consumer"}),
                "consumer": frozenset(),
                "board": frozenset(),
                "bridge": frozenset(),
            },
        )
        self.units = [
            plan.Unit(id="rust.core", jobs=["clippy", "test"], package="core", fixtures=True),
            plan.Unit(id="rust.leaf", jobs=["clippy", "test"], package="leaf"),
            plan.Unit(id="rust.consumer", jobs=["clippy", "test"], package="consumer"),
            plan.Unit(id="rust.board", jobs=["embedded"], package="board"),
            plan.Unit(id="rust.bridge", jobs=["clippy", "test", "wasm-bridges"], package="bridge"),
            unit("web.browser", ["web"], triggers=("builder/app/**", "specs/vectors/**")),
            unit("swift.kit", ["ios-unit"], triggers=("ios/**", "specs/vectors/**")),
            unit("ci.builds", ["embedded"], foundation=True),
            unit("ci.docs", ["docs"], route="required", triggers=("docs/**",)),
            unit("manual.writer", [], route="manual"),
            unit("missing.route", [], triggers=("demo/**",)),
        ]

    def plan_for(self, *paths, deleted=()):
        return plan.select(self.units, self.graph, list(paths), deleted=deleted)

    def selected(self, *paths):
        return {item.id for item in self.plan_for(*paths).selected}

    def test_path_and_graph_cases(self) -> None:
        cases = {
            "crates/leaf/src/lib.rs": {"rust.leaf", "rust.consumer"},
            "crates/core/src/lib.rs": {"rust.core", "rust.leaf", "rust.consumer", "rust.board"},
            "crates/core/tests/common/mod.rs": {"rust.core", "rust.leaf", "rust.consumer"},
            "crates/core/Cargo.toml": {"rust.core", "rust.leaf"},
            "specs/vectors/format.json": {"web.browser", "swift.kit"},
            "builder/app/src/panel.ts": {"web.browser"},
            "docs/guide.md": {"ci.docs"},
            "fixtures/sources/tile.bin": {"rust.core"},
            "firmware/board/src/main.rs": {"rust.board"},
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertTrue(expected.issubset(self.selected(path)), path)

    def test_a_standalone_root_is_a_reverse_dependency_like_any_other(self) -> None:
        reasons = next(item.reasons for item in self.plan_for("crates/core/src/lib.rs").units if item.id == "rust.board")
        self.assertTrue(any("reverse dependency board compiles core" in reason for reason in reasons))
        # …but it never becomes a `-p` argument of the root-workspace run.
        self.assertNotIn("board", plan.select(self.units, self.graph, ["crates/core/src/lib.rs"]).packages)

    def test_a_wasm_producer_reaches_its_browser_job(self) -> None:
        self.assertEqual(
            plan.required_jobs(self.plan_for("crates/bridge/src/lib.rs")),
            ["clippy", "selection", "test", "wasm-bridges"],
        )

    def test_shared_vectors_reach_every_client(self) -> None:
        self.assertEqual(self.selected("specs/vectors/format.json"), {"web.browser", "swift.kit"})

    def test_a_foundation_change_selects_the_whole_relevant_graph(self) -> None:
        for path in ("Cargo.toml", "rust-toolchain.toml", ".cargo/config.toml"):
            with self.subTest(path=path):
                selected = self.selected(path)
                self.assertIn("rust.core", selected)
                self.assertIn("ci.builds", selected)
        # A root manifest change is the root workspace; a toolchain change is every root.
        self.assertNotIn("rust.board", self.selected("Cargo.toml"))
        self.assertIn("rust.board", self.selected("rust-toolchain.toml"))

    def test_an_unknown_production_path_is_an_error(self) -> None:
        self.assertRegex(self.plan_for("unknown/new_source.py").errors[0], "no owner")
        self.assertFalse(self.plan_for("unknown/notes.md").errors)

    def test_a_selected_suite_without_a_job_is_an_error(self) -> None:
        errored = self.plan_for("demo/src/index.ts")
        self.assertIn("missing.route", {item.id for item in errored.selected})
        self.assertTrue(any("no executable CI route" in error for error in errored.errors))

    def test_a_deleted_path_with_no_owner_runs_the_whole_graph_instead_of_nothing(self) -> None:
        self.units.append(unit("ci.ui-snapshots", ["ui-snapshots"], triggers=("render/**",)))
        deleted = self.plan_for("crates/gone/src/lib.rs", deleted={"crates/gone/src/lib.rs"})
        self.assertFalse(deleted.errors)
        selected = {item.id for item in deleted.selected}
        self.assertIn("rust.core", selected)
        self.assertIn("ci.docs", selected)
        # The deleted file may have been a rendering input, so "whole graph" includes the sweep.
        self.assertIn("ci.ui-snapshots", selected)

    def test_many_unowned_deletions_share_one_reason(self) -> None:
        # A reason per path multiplies by every package and every declared suite. The `ci`
        # gate reads the whole plan from one environment variable, which the shell refuses
        # to start with past 128 KiB, so a branch that deletes a directory must not grow it.
        gone = {f"scratch/tool/src/bin/b{index}.rs" for index in range(40)}
        emptied = self.plan_for(*sorted(gone), deleted=gone)
        self.assertFalse(emptied.errors)
        whole = {
            reason
            for item in emptied.selected
            for reason in item.reasons
            if reason.startswith(plan.WHOLE_GRAPH)
        }
        self.assertEqual(len(whole), 1)
        self.assertIn("40 deleted paths", whole.pop())

    def test_a_rename_selects_the_owner_it_left_and_the_owner_it_joined(self) -> None:
        moved = self.plan_for("crates/leaf/src/moved.rs", "crates/core/src/moved.rs", deleted={"crates/leaf/src/moved.rs"})
        self.assertFalse(moved.errors)
        self.assertEqual(moved.packages, ["consumer", "core", "leaf"])

    def test_an_explicitly_invoked_suite_is_never_selected(self) -> None:
        self.units.append(unit("manual.fixture", [], route="manual", triggers=("fixtures/sources/**",)))
        chosen = self.plan_for("fixtures/sources/tile.bin")
        self.assertFalse(chosen.errors)
        self.assertNotIn("manual.fixture", {item.id for item in chosen.selected})

    def test_release_selects_every_routed_suite_but_no_sweep_or_live_work(self) -> None:
        self.units.append(unit("live.service", [], route="live"))
        released = plan.select_release(self.plan_for("README.md"))
        selected = {item.id for item in released.selected}
        self.assertIn("rust.core", selected)
        self.assertIn("swift.kit", selected)
        self.assertNotIn("missing.route", selected)
        self.assertNotIn("manual.writer", selected)
        self.assertNotIn("live.service", selected)
        self.assertEqual(released.errors, [])

    def test_a_release_still_respects_the_snapshot_sweep_budget(self) -> None:
        self.units.append(unit("ci.ui-snapshots", ["ui-snapshots"], triggers=("render/**",)))
        released = plan.select_release(self.plan_for("README.md"))
        self.assertNotIn("ci.ui-snapshots", {item.id for item in released.selected})

    def test_the_json_plan_reports_jobs_platforms_and_unselected_reasons(self) -> None:
        self.units[5].platforms = ("linux", "macos")
        data = plan.plan_data(self.plan_for("builder/app/src/panel.ts"))
        browser = next(item for item in data["suites"] if item["id"] == "web.browser")
        self.assertEqual(browser["platforms"], ["linux", "macos"])
        self.assertEqual(browser["jobs"], ["web"])
        swift = next(item for item in data["suites"] if item["id"] == "swift.kit")
        self.assertEqual(swift["reasons"], [plan.NOT_SELECTED])
        # The job list closes over the prerequisite graph, so producers still run.
        self.assertEqual(data["required_jobs"], ["selection", "wasm-bridges", "web"])

    def test_changed_paths_come_from_a_real_git_range(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for command in (
                ["git", "init", "-q"],
                ["git", "config", "user.email", "tests@example.com"],
                ["git", "config", "user.name", "Tests"],
            ):
                subprocess.run(command, cwd=root, check=True)
            source = root / "web/src/view.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export const value = 1;\n", encoding="utf-8")
            (root / "web/src/gone.ts").write_text("export const old = 1;\n", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            subprocess.run(["git", "commit", "-qm", "base"], cwd=root, check=True)
            base = subprocess.run(
                ["git", "rev-parse", "HEAD"], cwd=root, check=True, capture_output=True, text=True
            ).stdout.strip()
            source.write_text("export const value = 2;\n", encoding="utf-8")
            (root / "web/src/gone.ts").unlink()
            subprocess.run(["git", "commit", "-qam", "head"], cwd=root, check=True)
            changed, deleted = plan.git_changed_paths(root, base, "HEAD")
            self.assertEqual(changed, ["web/src/gone.ts", "web/src/view.ts"])
            self.assertEqual(deleted, {"web/src/gone.ts"})


class ExecutionTests(unittest.TestCase):
    """What the plan runs locally, and what it refuses to run."""

    def setUp(self) -> None:
        self.graph = plan.CargoGraph(
            {
                "core": package("core", "crates/core", ordinary=("main", "obc_core"), fixtures=("captured",)),
                "board": package("board", "firmware/board", product_root="firmware/board"),
            },
            {"core": frozenset(), "board": frozenset()},
        )
        self.units = [
            plan.Unit(id="rust.core", jobs=["test"], package="core"),
            plan.Unit(id="rust.board", jobs=["embedded"], package="board", command="cargo build --board"),
            unit("ci.docs", ["docs"], route="required"),
        ]

    def test_the_tier_split_is_the_required_features_gate(self) -> None:
        self.assertEqual(
            plan.cargo_filter(self.graph, "fast"),
            "(package(=core) & binary(=main)) | (package(=core) & binary(=obc_core))",
        )
        self.assertEqual(plan.cargo_filter(self.graph, "fixtures"), "(package(=core) & binary(=captured))")
        # A carved target belongs to an explicitly invoked suite, so no tier compiles it.
        self.assertEqual(
            plan.cargo_filter(self.graph, "fast", carved={("core", "main")}),
            "(package(=core) & binary(=obc_core))",
        )
        with self.assertRaisesRegex(plan.PlanError, "no Rust binaries"):
            plan.cargo_filter(self.graph, "fast", packages=[])

    def test_an_empty_package_set_produces_no_cargo_invocation(self) -> None:
        empty = plan.select(self.units, self.graph, ["docs/guide.md"])
        with patch("builtins.print") as printed:
            self.assertEqual(plan.run_plan(empty, self.graph, Path("."), dry_run=True), 0)
        printed_lines = " ".join(str(call.args[0]) for call in printed.call_args_list if call.args)
        self.assertNotIn("cargo", printed_lines)
        self.assertNotIn("--workspace", printed_lines)

    def test_a_selected_package_becomes_package_flags_and_a_filter(self) -> None:
        chosen = plan.select(self.units, self.graph, ["crates/core/src/lib.rs"])
        with patch("builtins.print") as printed:
            plan.run_plan(chosen, self.graph, Path("."), dry_run=True)
        printed_lines = " ".join(str(call.args[0]) for call in printed.call_args_list if call.args)
        self.assertIn("-p core", printed_lines)
        self.assertNotIn("--workspace", printed_lines)
        # A narrowed set can hold only packages that declare no test yet; that is not a
        # failed run. Only the workspace run in tools/ci/test.sh treats it as one.
        self.assertIn("--no-tests warn", printed_lines)

    def test_a_selection_error_publishes_no_plan(self) -> None:
        """The `selection` job exits nonzero and prints nothing the aggregate can read."""

        arguments = plan.build_parser().parse_args(["select", "--base", "x"])
        arguments.jobs_file = None
        with patch.object(plan, "load", return_value=(self.graph, {}, self.units)), \
             patch.object(plan, "git_changed_paths", return_value=(["unknown/source.py"], set())), \
             patch("builtins.print") as printed:
            self.assertEqual(plan.command_select(arguments), 1)
        self.assertIn("no owner", " ".join(str(call.args[0]) for call in printed.call_args_list if call.args))

    def test_a_selection_error_runs_nothing(self) -> None:
        errored = plan.select(self.units, self.graph, ["unknown/source.py"])
        with patch("builtins.print"), patch("sys.stderr"):
            self.assertEqual(plan.run_plan(errored, self.graph, Path("."), dry_run=True), 1)

    def test_gates_name_jobs_and_a_ci_only_suite_is_never_claimed(self) -> None:
        units = [
            plan.Unit(id="rust.core", jobs=["test"], package="core"),
            unit("ci.portability", ["clippy"], route="required", ci_only=True),
        ]
        self.assertEqual([item.id for item in plan.reproduced(units, ["test"])], ["rust.core"])
        self.assertEqual(plan.reproduced(units, ["clippy"]), [])
        with self.assertRaisesRegex(plan.PlanError, "unknown gate"):
            plan.gate_jobs(["nonexistent"])


class ValidationTests(unittest.TestCase):
    """The document rejections that protect the derived facts."""

    TABLE = {"test": plan.Job(roots=(".",), script="tools/ci/test.sh")}

    def errors(self, document):
        graph = plan.CargoGraph(
            {"core": package("core", "crates/core", examples=("writer",))}, {"core": frozenset()}
        )
        with patch.object(plan, "JOBS", self.TABLE):
            return plan.validate(plan.repository_root(), graph, document, [])

    def test_a_carved_target_must_exist(self) -> None:
        suite = {"id": "manual.writer", "route": "manual", "command": "cargo run", "package": "core"}
        self.assertEqual(self.errors({"suite": [dict(suite, targets=["writer"])]}), [])
        self.assertIn(
            "manual.writer: core has no target ghost",
            self.errors({"suite": [dict(suite, targets=["ghost"])]}),
        )

    def test_a_job_script_must_exist(self) -> None:
        with patch.object(plan, "JOBS", {"test": plan.Job(roots=(".",), script="tools/ci/missing.sh")}):
            graph = plan.CargoGraph({"core": package("core", "crates/core")}, {"core": frozenset()})
            self.assertIn(
                "job test names a missing script tools/ci/missing.sh",
                plan.validate(plan.repository_root(), graph, {"suite": []}, []),
            )


class WorkflowStructureTests(unittest.TestCase):
    """The parsed-YAML check, against a synthetic workflow."""

    WORKFLOW = """
name: CI
on:
  pull_request:
jobs:
  selection:
    runs-on: ubuntu-latest
    steps:
      - run: python3 tools/test_plan.py select --base x
  guards:
    runs-on: ubuntu-latest
    steps:
      - run: python3 tools/check_one_home.py
  test:
    needs: selection
    if: contains(fromJSON(needs.selection.outputs.jobs), 'test')
    runs-on: ubuntu-latest
    steps:
      - run: bash tools/ci/test.sh nextest-fast
  ci:
    if: always()
    needs: [selection, guards, test]
    runs-on: ubuntu-latest
    steps:
      - run: python3 tools/ci_aggregate.py
"""

    TABLE = {
        "selection": plan.Job(unconditional=True),
        "guards": plan.Job(unconditional=True),
        "test": plan.Job(needs=("selection",)),
    }

    def check(self, workflow: str) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / ".github/workflows/ci.yml"
            path.parent.mkdir(parents=True)
            path.write_text(workflow, encoding="utf-8")
            with patch.object(plan, "JOBS", self.TABLE):
                return plan.validate_workflow(root)

    def test_a_matching_workflow_passes(self) -> None:
        self.assertEqual(self.check(self.WORKFLOW), [])

    def test_structural_rejections(self) -> None:
        cases = [
            ("job gates on another name", ("jobs), 'test')", "jobs), 'testx')"), "gates on another job"),
            ("job outside the aggregate", ("needs: [selection, guards, test]", "needs: [selection, guards]"), "aggregate gate"),
            ("job with no runner", ("    runs-on: ubuntu-latest\n    steps:\n      - run: bash", "    steps:\n      - run: bash"), "no runner image"),
            ("ungated conditional job", ("    if: contains(fromJSON(needs.selection.outputs.jobs), 'test')\n", ""), "not gated"),
            ("needs graph disagrees", ("  test:\n    needs: selection", "  test:\n    needs: [selection, guards]"), "the job table says"),
            ("workflow job outside the table", ("  guards:\n", "  extra:\n"), "not in the job table"),
        ]
        for name, (before, after), expected in cases:
            with self.subTest(case=name):
                errors = self.check(self.WORKFLOW.replace(before, after))
                self.assertTrue(any(re.search(expected, error) for error in errors), errors)

    def test_a_missing_job_has_no_workflow_route(self) -> None:
        errors = self.check(self.WORKFLOW.replace("  test:\n", "  other:\n"))
        self.assertTrue(any("has no workflow route" in error for error in errors), errors)


class ShippedPlanTests(unittest.TestCase):
    """The shipped plan's own routing, so a real change class cannot drift silently."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.root = plan.repository_root()
        cls.graph, cls.document, cls.units = plan.load(cls.root)
        cls.unconditional = {name for name, job in plan.JOBS.items() if job.unconditional}

    def jobs_for(self, *paths):
        chosen = plan.select(self.units, self.graph, list(paths))
        self.assertEqual(chosen.errors, [])
        return sorted(set(plan.required_jobs(chosen)) - self.unconditional)

    def test_the_documents_and_the_workflow_validate(self) -> None:
        self.assertEqual(plan.validate(self.root, self.graph, self.document, self.units), [])
        self.assertEqual(plan.validate_workflow(self.root), [])

    def test_every_package_reaches_the_jobs_that_compile_it(self) -> None:
        expected = {
            "obc-crc": ["clippy", "fmt", "test"],
            "obc-fw-nrf54l": ["embedded", "fmt"],
            "obc-boot": ["boot", "fmt"],
            "obc-desktop": ["desktop", "fmt"],
            "obc-web-convert": ["clippy", "fmt", "test", "wasm-bridges"],
            "obc-web-demo": ["clippy", "fmt", "test", "wasm"],
            "obc-pack": ["builder-python", "clippy", "fmt", "test"],
            "obc-sim": ["clippy", "fmt", "test", "ui-snapshots"],
            "obc-app": ["clippy", "device", "fmt", "test"],
        }
        for name, jobs in expected.items():
            with self.subTest(package=name):
                self.assertEqual(plan.package_jobs(self.graph, name), jobs)

    def test_selected_job_set_per_change_class(self) -> None:
        """The exact conditional job set per class. The last three classes are the crates
        whose only build is a non-Cargo command, which a subset assertion let regress once."""

        cases = [
            ("documentation only", ["docs/content/ride.md"], ["docs"]),
            # Agent prose instructs an agent; it decides nothing. It must not build every
            # platform, and the unconditional guards job still validates the policy.
            ("agent prose only", ["CLAUDE.md", "AGENTS.md"], ["docs"]),
            ("leaf Rust crate", ["host/obc-bench/src/main.rs"], ["clippy", "fmt", "test"]),
            (
                "foundational Rust crate",
                ["firmware/obc-crc/src/lib.rs"],
                ["boot", "builder-python", "clippy", "desktop", "desktop-frontend", "device", "embedded", "fmt", "test", "ui-snapshots", "wasm", "wasm-bridges"],
            ),
            (
                "shared vectors",
                ["specs/vectors/obcm-v2.json"],
                ["clippy", "device", "fmt", "ios-unit", "test", "wasm-bridges", "web"],
            ),
            (
                "desktop launch harness",
                ["apps/obc-desktop/e2e/launch.py"],
                ["desktop", "desktop-frontend", "desktop-launch", "fmt", "wasm-bridges"],
            ),
            ("iOS application", ["companion-ios/OBCCompanion/App.swift"], ["ios-app", "ios-release"]),
            (
                "web only",
                ["builder/app/src/lib/panel.ts"],
                ["desktop", "desktop-frontend", "desktop-launch", "fmt", "wasm-bridges", "web", "web-browser"],
            ),
            (
                "workflow",
                [".github/workflows/ci.yml"],
                ["boot", "builder-python", "clippy", "deny", "desktop", "desktop-frontend", "desktop-launch", "device", "docs", "embedded", "fmt", "ios-app", "ios-release", "ios-unit", "test", "ui-snapshots", "verification", "wasm", "wasm-bridges", "web", "web-browser"],
            ),
            (
                "nextest configuration",
                [".config/nextest.toml"],
                ["boot", "builder-python", "clippy", "deny", "desktop", "desktop-frontend", "desktop-launch", "device", "docs", "embedded", "fmt", "ios-app", "ios-release", "ios-unit", "test", "ui-snapshots", "verification", "wasm", "wasm-bridges", "web", "web-browser"],
            ),
            ("web demo crate", ["apps/obc-web-demo/src/lib.rs"], ["clippy", "fmt", "test", "wasm"]),
            ("web demo Trunk target", ["docs/index.html"], ["docs", "wasm", "wasm-bridges"]),
            (
                "web demo browser harness",
                ["apps/obc-web-demo/tests/browser/ride-log.test.js"],
                ["clippy", "fmt", "test", "wasm"],
            ),
            (
                "OBCKit package source",
                ["companion-ios/Packages/OBCKit/Sources/OBCTransport/BLE/Client.swift"],
                ["ios-app", "ios-release", "ios-unit"],
            ),
            (
                "repository tooling",
                ["tools/fixtures.py"],
                ["desktop", "desktop-frontend", "test", "wasm-bridges"],
            ),
        ]
        for name, paths, expected in cases:
            with self.subTest(change=name):
                self.assertEqual(self.jobs_for(*paths), expected)

    def test_a_foundation_change_selects_the_whole_relevant_graph(self) -> None:
        whole = self.jobs_for(".github/workflows/ci.yml")
        for path in ("tools/test_plan.py", "tools/ci/test.sh", "testing/suites.toml"):
            with self.subTest(path=path):
                self.assertEqual(self.jobs_for(path), whole)
        for path in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rustfmt.toml", ".cargo/config.toml"):
            with self.subTest(path=path):
                self.assertLessEqual(
                    {"boot", "clippy", "desktop", "device", "embedded", "fmt", "test", "wasm", "wasm-bridges"},
                    set(self.jobs_for(path)),
                )

    def test_the_snapshot_sweep_requires_its_own_rendering_inputs(self) -> None:
        cases = [
            ("testing/coverage-policy.toml", False),
            (".github/workflows/ci.yml", False),
            ("firmware/obc-render/src/stroke.rs", True),
            ("firmware/ui-snapshots.sha256", True),
            ("firmware/obc-app/i18n/en.toml", True),
            ("firmware/obc-app/assets/landmark.bin", True),
            ("firmware/obc-render/fonts/terminus/font.bdf", True),
        ]
        for path, selected in cases:
            with self.subTest(path=path):
                chosen = plan.select(self.units, self.graph, [path])
                sweep = next(item for item in chosen.units if item.id == "ci.ui-snapshots")
                self.assertEqual(sweep.selected, selected)

    def test_the_captured_fixture_tier_is_the_six_carved_targets(self) -> None:
        self.assertEqual(
            sorted(
                f"{name}:{target}"
                for name, item in self.graph.packages.items()
                for target in item.fixture_targets
            ),
            [
                "obc-app:peak_view_photos",
                "obc-dem:assets",
                "obc-host-core:altitude_fusion",
                "obc-reader:poi_fixtures",
                "obc-route:nav_fixtures",
                "obc-sim:present_fixtures",
            ],
        )

    def test_explicitly_invoked_targets_are_in_no_tier(self) -> None:
        expression = plan.cargo_filter(self.graph, "fast", carved=plan.carved_targets(self.document))
        for carved in ("binary(=decode)", "binary(=real_tile)", "binary(=assistant_places)"):
            self.assertNotIn(carved, expression)

    def test_every_required_suite_a_full_local_run_misses_is_named(self) -> None:
        covered = {item.id for item in plan.reproduced(self.units, sorted(plan.GATES))}
        missing = sorted(item.id for item in self.units if item.route == "required" and item.id not in covered)
        self.assertEqual(
            missing,
            [
                "ci.card-scheduler-guard",
                "ci.catalog-ownership-guard",
                "ci.changelog",
                "ci.fixture-policy",
                "ci.ios-host-portability",
                "ci.one-home-guard",
                "ci.prose",
                "ci.render-key-guard",
                "ci.retired-map-stack",
                "ci.screen-vocabulary-guard",
                "python.repository-tools",
            ],
        )


class JobPackageTableTests(unittest.TestCase):
    """The job table states what the builders compile; nothing derives it, so pin it here."""

    root = plan.repository_root()

    def package_name(self, manifest_directory: Path) -> str:
        with (manifest_directory / "Cargo.toml").open("rb") as handle:
            return tomllib.load(handle)["package"]["name"]

    def test_wasm_bridges_matches_the_build_script(self) -> None:
        script = (self.root / "builder/build-wasm-bridges.sh").read_text(encoding="utf-8")
        directories = re.findall(r"\bwasm-pack\s+build\s+(\S+)", script)
        self.assertEqual(
            {self.package_name(self.root / directory) for directory in directories},
            set(plan.JOBS["wasm-bridges"].packages),
        )

    def test_wasm_matches_the_trunk_target_page(self) -> None:
        config = self.root / "docs/Trunk.toml"
        with config.open("rb") as handle:
            page = config.parent / tomllib.load(handle)["build"]["target"]
        hrefs = re.findall(
            r'<link[^>]*data-trunk[^>]*rel="rust"[^>]*href="([^"]+)"', page.read_text(encoding="utf-8")
        )
        self.assertLessEqual(
            {self.package_name((page.parent / href).parent) for href in hrefs},
            set(plan.JOBS["wasm"].packages),
        )


if __name__ == "__main__":
    unittest.main()
