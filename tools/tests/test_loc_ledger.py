import contextlib
import importlib.util
import io
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "loc_ledger.py"
SPEC = importlib.util.spec_from_file_location("loc_ledger", MODULE_PATH)
ledger = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = ledger
SPEC.loader.exec_module(ledger)


class _NoTree:
    """A tree that knows nothing — isolates the path rules from rule 7."""

    @staticmethod
    def declared_under_cfg_test(_path: str) -> bool:
        return False


def classify(path: str) -> tuple[str, str]:
    return ledger.classify_file(path, _NoTree)


class ClassificationTests(unittest.TestCase):
    def test_ordinary_crate_source_is_production(self):
        self.assertEqual(classify("firmware/obc-storage/src/flat/store.rs")[0], ledger.PRODUCTION)

    def test_integration_tests_and_benches_are_support(self):
        for path in (
            "firmware/obc-link/tests/flat_harness/mod.rs",
            "host/obc-pack/benches/pack.rs",
            "firmware/obc-fw-nrf54l/src/bin/flat_store_bench.rs",
            "host/obc-pack/src/serialize_test.rs",
        ):
            self.assertEqual(classify(path)[0], ledger.TEST, path)

    def test_oracle_crates_are_support_everywhere(self):
        self.assertEqual(classify("host/obcm-testkit/src/lib.rs")[0], ledger.TEST)
        self.assertEqual(classify("host/obc-vectors/src/main.rs")[0], ledger.TEST)

    def test_non_rust_is_uncounted_rather_than_test(self):
        for path in ("specs/FLAT_Store_Format.md", ".github/workflows/ci.yml"):
            self.assertEqual(classify(path)[0], ledger.OTHER, path)

    def test_reason_names_the_rule_that_matched(self):
        self.assertIn("rule 1", classify("firmware/obc-link/tests/harness.rs")[1])


class CfgExpressionTests(unittest.TestCase):
    """Only an exact `#[cfg(test)]` gate excludes an item."""

    def test_cfg_attr_is_never_a_gate(self):
        """`cfg_attr` applies an attribute conditionally; the item compiles either
        way, so the scanner must not even consider it."""
        for attr in ("#![cfg_attr(not(test), no_std)]", "#[cfg_attr(test, derive(Debug))]"):
            self.assertIsNone(ledger._CFG_ATTR.match(attr), attr)
        gated, _ = ledger.scan_cfg_test("#[cfg_attr(test, derive(Debug))]\nstruct S;\n")
        self.assertEqual(gated, set())

    def test_a_no_std_crate_root_is_not_a_test_file(self):
        """The live bug: `#![cfg_attr(not(test), no_std)]` gated whole crates."""
        src = "\n".join(
            [
                "//! A crate.",  # 1
                "#![cfg_attr(not(test), no_std)]",  # 2
                "",  # 3
                "#[cfg(test)]",  # 4
                "extern crate self as thing;",  # 5
                "",  # 6
                "pub fn production() {}",  # 7
            ]
        )
        gated, _ = ledger.scan_cfg_test(src)
        self.assertEqual(gated, {4, 5})


class CfgTestScannerTests(unittest.TestCase):
    def test_trailing_test_module_is_the_only_gated_region(self):
        src = "\n".join(
            [
                "pub fn one() -> u32 {",  # 1
                "    1",  # 2
                "}",  # 3
                "",  # 4
                "#[cfg(test)]",  # 5
                "mod tests {",  # 6
                "    fn t() {}",  # 7
                "}",  # 8
            ]
        )
        gated, mods = ledger.scan_cfg_test(src)
        self.assertEqual(gated, {5, 6, 7, 8})
        self.assertEqual(mods, set())

    def test_a_brace_in_a_string_or_comment_does_not_close_the_block(self):
        src = "\n".join(
            [
                "#[cfg(test)]",  # 1
                "mod tests {",  # 2
                '    const S: &str = "}";',  # 3
                "    // }",  # 4
                "    fn t() {}",  # 5
                "}",  # 6
                "pub fn after() {}",  # 7
            ]
        )
        gated, _ = ledger.scan_cfg_test(src)
        self.assertEqual(gated, {1, 2, 3, 4, 5, 6})

    def test_only_an_exact_gate_declares_a_rule_7_module(self):
        """`any(test, feature = "std")` does not prove an item is test-only, so
        `sim` stays production here and is excluded by name instead."""
        src = "#[cfg(test)]\nmod granularity;\n#[cfg(any(test, feature = \"std\"))]\npub mod sim;\n"
        gated, mods = ledger.scan_cfg_test(src)
        self.assertEqual(mods, {"granularity"})
        self.assertEqual(gated, {1, 2})

    def test_a_semicolon_inside_brackets_does_not_end_the_item(self):
        src = "\n".join(
            [
                "#[cfg(test)]",  # 1
                "fn helper(buf: [u8; 4]) {",  # 2
                "    let _ = buf;",  # 3
                "}",  # 4
                "pub fn production() {}",  # 5
            ]
        )
        gated, _ = ledger.scan_cfg_test(src)
        self.assertEqual(gated, {1, 2, 3, 4})

    def test_a_lifetime_tick_is_not_a_char_literal(self):
        tick = "'"
        src = "\n".join(
            [
                "#[cfg(test)]",  # 1
                "mod t {",  # 2
                f"    impl<{tick}a> Foo<{tick}a> for Bar<{tick}a> {{",  # 3
                "        fn f(&self) {}",  # 4
                "    }",  # 5
                "}",  # 6
                "pub fn production() {}",  # 7
            ]
        )
        gated, _ = ledger.scan_cfg_test(src)
        self.assertEqual(gated, {1, 2, 3, 4, 5, 6})

    def test_a_cfg_without_test_is_production(self):
        src = "#[cfg(feature = \"std\")]\nmod host {\n    fn f() {}\n}\n"
        gated, mods = ledger.scan_cfg_test(src)
        self.assertEqual(gated, set())
        self.assertEqual(mods, set())


class CodeLineTests(unittest.TestCase):
    def test_blanks_and_comments_are_not_code(self):
        lines = ["fn f() {", "    // a comment", "", "    /* block", "    still */", "    1", "}"]
        stripped = ledger.strip_noise(lines)
        code = [ledger.is_code_line(s) for s in stripped]
        self.assertEqual(code, [True, False, False, False, False, True, True])


class StorageTotalTests(unittest.TestCase):
    """One real repository, so the tree walk and the buckets are exercised together."""

    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.repo = Path(self.dir.name)
        self.addCleanup(self.dir.cleanup)
        self.vcs("init", "-q", "-b", "main")
        # Detached maintenance must not outlive this temporary repository.
        self.vcs("config", "maintenance.auto", "false")
        self.vcs("config", "user.email", "t@example.com")
        self.vcs("config", "user.name", "T")
        self.write("src/lib.rs", "pub mod thing;\n")
        self.vcs("add", "-A")
        self.vcs("commit", "-qm", "base")
        self.base = self.vcs("rev-parse", "HEAD").strip()

    def vcs(self, *args: str) -> str:
        return subprocess.run(
            ["git", "-C", str(self.repo), *args],
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        ).stdout

    def write(self, rel: str, text: str) -> None:
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def test_the_count_reconciles_one_committed_tree(self):
        root = "firmware/obc-storage/src/flat/"
        self.write(root + "mod.rs", 'pub mod store;\n#[cfg(test)]\nmod check;\n'
                   '#[cfg(any(test, feature = "std"))]\npub mod host;\n')
        self.write(root + "store.rs", '// production comment\n\npub fn f() {}\n'
                   '#[cfg(test)]\nmod tests { fn t() {} }\npub fn last() {}')
        self.write(root + "check.rs", "fn helper() {}\n")
        self.write(root + "host.rs", "pub fn production_host() {}\n")
        self.write(root + "sim.rs", "pub fn fault_model() {}\n")
        # Outside the pinned scope: the board's adapter is not counted.
        self.write("firmware/obc-fw-nrf54l/src/flat_store.rs", "pub fn adapter() {}\n")
        self.vcs("add", "-A")
        self.vcs("commit", "-qm", "storage")
        head = self.vcs("rev-parse", "HEAD").strip()
        self.write(root + "store.rs", "uncommitted replacement\n")
        total = ledger.storage_total(str(self.repo), head)
        self.assertEqual(total.head, head)
        self.assertEqual(total.totals(ledger.PRODUCTION).raw, 8)
        self.assertEqual(total.totals(ledger.PRODUCTION).code, 6)
        self.assertEqual(total.totals(ledger.TEST).raw, 6)
        self.assertEqual(len(total.files), 5)
        report = ledger.render_storage_total(total)
        self.assertIn("8 production + 6 excluded = 14 Rust source lines", report)
        self.assertIn("within by 5992", report)
        self.assertEqual(report, ledger.render_storage_total(ledger.storage_total(str(self.repo), head)))
        with self.assertRaisesRegex(SystemExit, "scope contains no Rust"):
            ledger.storage_total(str(self.repo), self.base)

    def test_the_budget_check_uses_raw_lines_and_an_exact_limit(self):
        path = "firmware/obc-storage/src/flat/store.rs"
        for count in (6000, 6001):
            self.write(path, "// production comment\n" * count)
            self.vcs("add", "-A")
            self.vcs("commit", "-qm", f"{count} lines")
            # Exercise the real CLI against the temporary committed repository.
            with mock.patch.object(ledger, "__file__", str(self.repo / "loc_ledger.py")):
                with contextlib.redirect_stdout(io.StringIO()) as report:
                    status = ledger.main(["--storage-total", "--check-budget"])
                self.assertEqual(status, int(count > 6000))
                self.assertIn(f"Budget: {count} / 6000", report.getvalue())
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(ledger.main(["--storage-total"]), 0)
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                ledger.main([])  # --storage-total is the only mode


class CliTests(unittest.TestCase):
    """`main` always resolves the repository from the script's own location, so
    it is smoke-tested against this repository."""

    def test_main_counts_this_repository_and_exits_zero(self):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = ledger.main(["--storage-total"])
        out = buf.getvalue()
        self.assertEqual(code, 0)
        self.assertIn("Flat-store absolute line budget", out)
        self.assertIn("firmware/obc-storage/src/flat/store.rs", out)


if __name__ == "__main__":
    unittest.main()
