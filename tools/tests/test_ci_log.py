"""Tests for `tools/ci_log.py`: a raw job log reduces to the lines before its error markers."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ci_log

STAMP = "2026-01-01T00:00:00.0000000Z "
LOG = "".join(
    STAMP + line + "\n"
    for line in (
        "##[group]Run bash tools/ci/test.sh nextest-fast",
        "   Compiling obc-app v0.1.0",
        "\x1b[32;1m        PASS\x1b[0m [   0.011s] obc-app a::passes",
        "        FAIL [   0.011s] obc-app a::fails",
        "thread 'a::fails' panicked at src/a.rs:3:5:",
        "assertion failed: false",
        "     Summary [   0.5s] 2 tests run: 1 passed, 1 failed",
        "##[error]Process completed with exit code 100.",
        "##[group]Run python3 -m coverage lcov",
        "##[error]Process completed with exit code 1.",
    )
)


class ExcerptTests(unittest.TestCase):
    def test_timestamps_colour_and_progress_lines_are_gone(self):
        (window,) = ci_log.excerpts(LOG, lines=100)
        self.assertEqual(window[0], "##[group]Run bash tools/ci/test.sh nextest-fast")
        self.assertNotIn("   Compiling obc-app v0.1.0", window)
        self.assertFalse(any("PASS" in line for line in window))
        self.assertIn("assertion failed: false", window)

    def test_the_window_is_the_lines_before_the_marker_and_adjacent_windows_merge(self):
        (window,) = ci_log.excerpts(LOG, lines=2)
        self.assertEqual(
            window,
            [
                "assertion failed: false",
                "     Summary [   0.5s] 2 tests run: 1 passed, 1 failed",
                "##[error]Process completed with exit code 100.",
                "##[group]Run python3 -m coverage lcov",
                "##[error]Process completed with exit code 1.",
            ],
        )

    def test_a_log_without_a_marker_has_no_window(self):
        self.assertEqual(ci_log.excerpts(STAMP + "all fine\n", lines=5), [])


if __name__ == "__main__":
    unittest.main()
