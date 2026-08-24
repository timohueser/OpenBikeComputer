"""The `CoreMode` soak driver's log analysis, against recorded RTT text.

**Death trigger: delete with `tools/s5_core_mode_soak.py` when #1487 closes.** The script's whole
value is that a blind driver "passes" everything — a wedged VCOM, a `DEFMT_LOG` that swallows the
plan-start line, a map frame that lands *before* the banner. These pin the verdicts that catch each
of those, off recorded lines, so the rig is not itself the thing under test on the board.
"""

import unittest

from tools.s5_core_mode_soak import (
    DEEP_RIDE_MARGIN_MIN,
    STACK_RESERVE,
    assert_sequence,
    faults,
    margin_verdict,
    read_cycle,
    stack_peaks,
)

START = "0.100 DEBUG nav plan: start planner=0x2000a000 scratch=0x2000b000 tiles=0x2000c000"
BANNER = "0.150 INFO  ui frame: render 900 us + push 300 us (screen redraw, no map)"
MAP = "0.900 INFO  map frame: render 41000 us + push 3000 us | lod 2 | feat 900/1200 | chunks 8"
BUSY = "0.400 ERROR arena: render claim refused — held by Nav"


class CycleVerdicts(unittest.TestCase):
    def test_a_clean_cycle_passes(self):
        self.assertIsNone(read_cycle([START, BANNER, MAP]).verdict())
        self.assertIsNone(assert_sequence([START, BANNER, MAP]))

    def test_a_missing_plan_start_is_the_defmt_level_trap(self):
        """No `nav plan: start` means either the plan never armed or RTT is not at debug — and a
        driver that ignored it would report a clean run against a board that planned nothing."""
        why = read_cycle([BANNER, MAP]).verdict()
        self.assertIn("never armed", why)

    def test_a_freeze_with_no_banner_is_a_map_that_simply_stopped(self):
        why = read_cycle([START, MAP]).verdict()
        self.assertIn("no banner", why)

    def test_the_banner_is_one_repaint_per_freeze_not_one_per_pass(self):
        """The level→edge converter's whole point. Hundreds of ride-loop passes, one overlay
        repaint."""
        why = read_cycle([START, BANNER, BANNER, MAP]).verdict()
        self.assertIn("repainting per pass", why)

    def test_exactly_one_catch_up_repaint_after_the_answer(self):
        self.assertIn("0 full map repaints", read_cycle([START, BANNER]).verdict())
        self.assertIn("2 full map repaints", read_cycle([START, BANNER, MAP, MAP]).verdict())

    def test_an_arena_refusal_fails_the_cycle_however_it_ends(self):
        """**The failure this soak exists for**: a refused claim degrades silently on a shipping
        build, so the log line is the only witness."""
        why = read_cycle([START, BANNER, BUSY, MAP]).verdict()
        self.assertIn("arena refused", why)

    def test_a_map_frame_before_the_banner_means_the_freeze_did_not_hold(self):
        """Counts alone would pass this: one start, one banner, one map frame. The *order* is what
        says the map plane drew while the nav arm was still out."""
        self.assertIsNone(read_cycle([START, MAP, BANNER]).verdict())
        self.assertIn("freeze did not hold", assert_sequence([START, MAP, BANNER]))


class StackAndFaults(unittest.TestCase):
    def test_peaks_are_read_and_the_margin_floor_is_enforced(self):
        deep = STACK_RESERVE - DEEP_RIDE_MARGIN_MIN + 1
        lines = [f"1.0 INFO  stack high-water {deep} / {STACK_RESERVE} B (new peak)"]
        self.assertEqual(stack_peaks(lines), [deep])
        self.assertIn("under the", margin_verdict(stack_peaks(lines)))
        self.assertIsNone(margin_verdict([37_016]), "the pinned deep-ride peak still has its margin")

    def test_no_peak_reported_is_not_a_failure(self):
        self.assertIsNone(margin_verdict([]))

    def test_a_boot_fault_or_a_watchdog_reset_ends_a_soak(self):
        self.assertEqual(len(faults(["2.0 ERROR boot fault: MAP UNREADABLE", "3.0 INFO watchdog reset"])), 2)
        self.assertEqual(faults([START, BANNER, MAP]), [])


if __name__ == "__main__":
    unittest.main()
