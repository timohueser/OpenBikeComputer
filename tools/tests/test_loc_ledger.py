import importlib.util
import subprocess
import sys
import tempfile
import unittest
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
    return ledger.classify_file(path, _NoTree, _NoTree)


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

    def test_module_declarations_are_reported_for_rule_7(self):
        src = "#[cfg(test)]\nmod granularity;\n#[cfg(any(test, feature = \"std\"))]\npub mod sim;\n"
        gated, mods = ledger.scan_cfg_test(src)
        self.assertEqual(mods, {"granularity", "sim"})
        self.assertEqual(gated, {1, 2, 3, 4})

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


class EndToEndTests(unittest.TestCase):
    """One real repository, so the diff walk and the buckets are exercised together."""

    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.repo = Path(self.dir.name)
        self.addCleanup(self.dir.cleanup)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "t@example.com")
        self.git("config", "user.name", "T")
        self.write("src/lib.rs", "pub mod thing;\n#[cfg(test)]\nmod probe;\n")
        self.write("src/thing.rs", "pub fn a() -> u32 {\n    1\n}\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "base")
        self.base = self.git("rev-parse", "HEAD").strip()

    def git(self, *args: str) -> str:
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

    def test_production_test_and_uncounted_are_split(self):
        self.write(
            "src/thing.rs",
            "pub fn a() -> u32 {\n    // now two\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n",
        )
        self.write("src/probe.rs", "fn probe() {}\n")  # rule 7, via lib.rs
        self.write("README.md", "hello\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "change")
        head = self.git("rev-parse", "HEAD").strip()

        led = ledger.build_ledger(str(self.repo), self.base, head, "raw", None)
        # git aligns the trailing `}`, so the added production lines are the
        # comment, the new body line, a `}` and the blank before the test module.
        self.assertEqual(led.totals(ledger.PRODUCTION, "raw"), (4, 1, 3))
        # …of which the comment and the blank are not code.
        self.assertEqual(led.totals(ledger.PRODUCTION, "code"), (2, 1, 1))
        self.assertEqual(led.totals(ledger.TEST, "raw")[0], 3 + 1)  # cfg(test) block + probe.rs
        self.assertEqual(led.totals(ledger.OTHER, "raw")[0], 1)  # README.md, uncounted

        by_path = {f.path: f for f in led.files}
        self.assertIn("rule 7", by_path["src/probe.rs"].reason)
        self.assertTrue(by_path["src/thing.rs"].cfg_test_hunks)

    def test_deleted_production_file_counts_as_a_removal(self):
        (self.repo / "src/thing.rs").unlink()
        self.write("src/lib.rs", "#[cfg(test)]\nmod probe;\n")
        self.write("src/probe.rs", "fn probe() {}\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "delete")
        head = self.git("rev-parse", "HEAD").strip()

        led = ledger.build_ledger(str(self.repo), self.base, head, "raw", None)
        added, removed, net = led.totals(ledger.PRODUCTION, "raw")
        self.assertEqual(removed, 4)  # thing.rs (3) + the `pub mod thing;` line
        self.assertEqual(net, added - removed)
        self.assertLess(net, 0)

    def test_raw_totals_account_for_every_diff_line(self):
        """Nothing may fall between the buckets: raw adds/removes across all
        three must equal what `git diff --numstat` saw."""
        self.write(
            "src/thing.rs",
            "pub fn a() -> u32 {\n    2\n}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n",
        )
        self.write("src/probe.rs", "fn probe() {}\n")
        self.write("notes.md", "one\ntwo\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "change")
        head = self.git("rev-parse", "HEAD").strip()

        numstat = self.git("diff", "--numstat", "-M", self.base, head)
        want_add = sum(int(l.split("\t")[0]) for l in numstat.splitlines() if l)
        want_del = sum(int(l.split("\t")[1]) for l in numstat.splitlines() if l)

        led = ledger.build_ledger(str(self.repo), self.base, head, "raw", None)
        got_add = sum(led.totals(b, "raw")[0] for b in (ledger.PRODUCTION, ledger.TEST, ledger.OTHER))
        got_del = sum(led.totals(b, "raw")[1] for b in (ledger.PRODUCTION, ledger.TEST, ledger.OTHER))
        self.assertEqual((got_add, got_del), (want_add, want_del))

    def test_output_is_deterministic(self):
        self.write("src/thing.rs", "pub fn a() -> u32 {\n    2\n}\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "change")
        head = self.git("rev-parse", "HEAD").strip()
        runs = {
            ledger.render(
                ledger.build_ledger(str(self.repo), self.base, head, "raw", None), None, False
            )
            for _ in range(3)
        }
        self.assertEqual(len(runs), 1)


if __name__ == "__main__":
    unittest.main()
