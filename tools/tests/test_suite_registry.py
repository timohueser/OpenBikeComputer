from __future__ import annotations

import copy
import json
from pathlib import Path
import tempfile
import unittest

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import suite_registry as registry


class SuiteRegistryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("pub fn value() -> u8 { 1 }\n", encoding="utf-8")
        (self.root / "Cargo.toml").write_text("[package]\nname='demo'\nversion='0.1.0'\n", encoding="utf-8")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def metadata(self, _root: Path, manifest: Path | None) -> dict:
        name = "demo" if manifest is None else manifest.parent.name
        manifest_path = self.root / "Cargo.toml" if manifest is None else manifest
        return {
            "packages": [
                {
                    "name": name,
                    "manifest_path": str(manifest_path),
                    "targets": [{"name": name, "kind": ["lib"], "test": True}],
                }
            ]
        }

    def base_documents(self) -> tuple[dict, dict, list[registry.Discovered]]:
        suites = {
            "schema": 1,
            "suite": [
                {
                    "id": "demo",
                    "surface": "rust",
                    "level": "unit",
                    "command": "cargo test -p demo --lib --locked",
                    "fixtures": [],
                    "pull_request": "affected",
                    "scheduled": "none",
                    "extra_triggers": [],
                    "ownership": [{"kind": "rust-package", "name": "demo"}],
                    "coverage_component": "app",
                }
            ],
        }
        coverage = {
            "schema": 1,
            "component": [
                {"id": "app", "include": ["src/**"], "exclude": [], "enforcement": "report"}
            ],
        }
        discovered = [registry.Discovered("rust-target", "demo:demo", "Cargo.toml", "lib")]
        return suites, coverage, discovered

    def test_discovers_every_required_source(self) -> None:
        files = {
            "builder/app/src/example.test.ts": "test('x', () => {})",
            "tools/tests/test_tool.py": "def test_x(): pass",
            "firmware/tools/tests/test_firmware.py": "def test_x(): pass",
            "ops/weather/tests/test_weather.py": "def test_x(): pass",
            "builder/tests/test_builder.py": "def test_x(): pass",
            "tools/rain-radar-demo/tests/radar.test.ts": "test('x', () => {})",
            "companion-ios/OBCCompanionUITests/SmokeTests.swift": "func testSmoke() {}",
            "companion-ios/Packages/OBCKit/Package.swift": '.testTarget(name: "OBCKitTests")',
            "companion-ios/EchoHarness/Package.swift": "let package = Package(name: \"EchoHarness\")",
            ".github/workflows/ci.yml": "jobs:\n  test:\n    steps:\n      - run: cargo test --workspace --locked\n",
        }
        for relative, content in files.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        discovered = registry.discover_all(self.root, self.metadata)
        kinds = {item.kind for item in discovered}
        self.assertEqual(
            kinds,
            {
                "rust-target",
                "web-test",
                "python-test",
                "rain-radar-test",
                "xcuitest",
                "swift-target",
                "swift-package",
                "workflow-command",
            },
        )

    def test_list_data_is_derived_not_stored(self) -> None:
        suites, coverage, discovered = self.base_documents()
        inventory = registry.validate(self.root, suites, coverage, discovered)
        row = registry._suite_summary(inventory.suites[0], inventory.matches["demo"])
        self.assertEqual(row["discovered_units"], 1)

    def test_rejection_rules(self) -> None:
        cases = []

        def case(name, mutate, expected):
            cases.append((name, mutate, expected))

        case("duplicate id", lambda s, c, d: s["suite"].append(copy.deepcopy(s["suite"][0])), "duplicate suite IDs")
        case("orphan", lambda s, c, d: s["suite"][0].update(ownership=[{"kind": "rust-package", "name": "other"}]), "orphan discovered suite")
        case("invalid level", lambda s, c, d: s["suite"][0].update(level="integration"), "invalid level")
        case("invalid PR cadence", lambda s, c, d: s["suite"][0].update(pull_request="sometimes"), "invalid pull_request cadence")
        case("invalid schedule", lambda s, c, d: s["suite"][0].update(scheduled="monthly"), "invalid scheduled cadence")
        case("fixture declaration", lambda s, c, d: s["suite"][0].update(level="fixture"), "must declare fixtures")
        case("derived fact", lambda s, c, d: s["suite"][0].update(test_count=3), "derived fields are forbidden")
        case("budget issue", lambda s, c, d: s["suite"][0].update(budget_exception={"reason": "slow"}), "open GitHub issue")
        case("quarantine issue", lambda s, c, d: s["suite"][0].update(quarantine={"reason": "flake"}), "open GitHub issue")
        case("unknown component", lambda s, c, d: s["suite"][0].update(coverage_component="missing"), "unknown coverage component")
        case("dead entry", lambda s, c, d: s["suite"].append({**copy.deepcopy(s["suite"][0]), "id": "dead", "ownership": [{"kind": "workflow", "pattern": "never"}]}), "dead registry entry")
        case("missing trigger", lambda s, c, d: s["suite"][0].update(extra_triggers=["missing/**"]), "extra trigger matches no maintained path")
        case("live PR", lambda s, c, d: s["suite"][0].update(level="live", pull_request="always"), "cannot be required")
        case("live command", lambda s, c, d: s["suite"][0].update(command="curl https://example.com"), "appears to contact a live service")
        case("missing command path", lambda s, c, d: s["suite"][0].update(command="tools/missing.py check"), "command path does not exist")
        case("missing working directory", lambda s, c, d: s["suite"][0].update(command="cd missing && python3 -m unittest"), "working directory does not exist")
        case("bad safety policy", lambda s, c, d: c["component"].append({"id": "crc", "include": ["src/**"], "enforcement": "report"}), "must be planned as ratchet")
        case("invented baseline", lambda s, c, d: c["component"][0].update(baseline="pending"), "omit pending baseline")
        case("exclusion evidence", lambda s, c, d: c["component"][0].update(exclude=[{"path": "src/generated.rs"}]), "replacement evidence")

        for name, mutate, expected in cases:
            with self.subTest(name=name):
                suites, coverage, discovered = self.base_documents()
                mutate(suites, coverage, discovered)
                with self.assertRaisesRegex(registry.RegistryError, expected):
                    registry.validate(self.root, suites, coverage, discovered)

    def test_duplicate_ownership_is_rejected(self) -> None:
        suites, coverage, discovered = self.base_documents()
        duplicate = copy.deepcopy(suites["suite"][0])
        duplicate["id"] = "demo-too"
        suites["suite"].append(duplicate)
        with self.assertRaisesRegex(registry.RegistryError, "duplicate ownership"):
            registry.validate(self.root, suites, coverage, discovered)

    def test_real_sleep_requires_an_open_issue_exception(self) -> None:
        source = self.root / "tools/tests/test_wait.py"
        source.parent.mkdir(parents=True)
        source.write_text("import time\ntime.sleep(1)\n", encoding="utf-8")
        suites, coverage, _ = self.base_documents()
        suites["suite"][0].update(
            surface="python",
            command="python3 -m unittest tools.tests.test_wait",
            ownership=[{"kind": "path", "source": "python-test", "pattern": "tools/tests/test_*.py"}],
        )
        discovered = [registry.Discovered("python-test", "tools/tests/test_wait.py", "tools/tests/test_wait.py")]
        with self.assertRaisesRegex(registry.RegistryError, "real sleep"):
            registry.validate(self.root, suites, coverage, discovered)
        suites["suite"][0]["sleep_exception"] = {"reason": "bounded watchdog", "issue": "#1236"}
        registry.validate(self.root, suites, coverage, discovered)

    def test_workflow_parser_handles_inline_blocks_and_matrix_commands(self) -> None:
        workflow = self.root / ".github/workflows/ci.yml"
        workflow.parent.mkdir(parents=True)
        workflow.write_text(
            "steps:\n"
            "  - run: npm test\n"
            "  - run: |\n"
            "      python3 tools/check.py\n"
            "      echo ignored\n"
            '  - { name: test, cmd: "cargo test --locked" }\n',
            encoding="utf-8",
        )
        commands = {item.name for item in registry.discover_workflow(self.root)}
        self.assertEqual(commands, {"npm test", "python3 tools/check.py", "cargo test --locked"})


if __name__ == "__main__":
    unittest.main()
