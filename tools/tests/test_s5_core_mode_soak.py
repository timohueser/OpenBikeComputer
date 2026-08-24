"""The `CoreMode` soak driver's log analysis, against recorded RTT text.

**Death trigger: delete with `tools/s5_core_mode_soak.py` when #1487 closes.** The script's whole
value is that a blind driver "passes" everything — a wedged VCOM, a `DEFMT_LOG` that swallows the
plan-start line, a map frame that lands while the nav arm is still out. These pin the verdicts that
catch each of those, off recorded lines, so the rig is not itself the thing under test on the board.

Every transcript below is **realistic**: it carries the `ui frame:` lines a real cycle emits for the
planning spinner and the menus. An earlier revision keyed the banner on `ui frame:` and so reported
three banner repaints for a healthy cycle; the fixtures are noisy on purpose so that cannot come
back.
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
    step_acks,
)

START = "0.100 DEBUG nav plan: start planner=0x2000a000 scratch=0x2000b000 tiles=0x2000c000"
# The spinner and the menus take the same non-map render branch as the banner and share this line.
SPINNER = "0.150 INFO  ui frame: render 900 us + push 300 us (screen redraw, no map)"
BANNER = "0.200 INFO  freeze: banner repaint rows 96..132"
ANSWER = "0.800 INFO  nav route: ok len=4210 total_ms=690 snap_ms=12 search_ms=540 emit_ms=90"
MAP = "0.900 INFO  map frame: render 41000 us + push 3000 us | lod 2 | feat 900/1200 | chunks 8"
BUSY = "0.400 ERROR arena: render claim refused — held by Nav"
STEP = "0.050 INFO  input: Step 1 on Map"
# What a wedged VCOM looks like: RTT keeps flowing from sensors and frames, so the log grows while
# not one injected tap has landed.
WEDGED = [MAP, "0.3 INFO  gps: fix 46562000 8337000", SPINNER]
REFUSED = "0.120 WARN  nav: cannot start a plan (a cable transfer holds the store) — answering the failure tier"

# What a healthy `N` cycle actually looks like: the spinner repaints several times while the planner
# steps, and no banner appears at all (no gesture engages the freeze on today's board).
HEALTHY = [START, SPINNER, SPINNER, SPINNER, ANSWER, SPINNER, MAP]


class CycleVerdicts(unittest.TestCase):
    def test_a_healthy_cycle_passes_with_its_spinner_repaints(self):
        """**The regression in the rig itself.** Three `ui frame:` lines are what a real cycle emits;
        counting them as banners failed every healthy cycle and hid the ordering check behind it."""
        cycle = read_cycle(HEALTHY)
        self.assertEqual(cycle.banner_frames, 0)
        self.assertEqual(cycle.outcome, "ok")
        self.assertIsNone(cycle.verdict())
        self.assertIsNone(assert_sequence(HEALTHY))

    def test_a_missing_plan_start_is_the_defmt_level_trap(self):
        """No `nav plan: start` means either the plan never armed or RTT is not at debug — and a
        driver that ignored it would report a clean run against a board that planned nothing."""
        self.assertIn("never armed", read_cycle([SPINNER, ANSWER, MAP]).verdict())

    def test_a_refused_plan_is_reported_as_the_refusal_not_as_a_missing_start(self):
        why = read_cycle([REFUSED]).verdict()
        self.assertIn("refused", why)
        self.assertIn("cable transfer", why)

    def test_a_search_that_never_answers_holds_the_arm_forever(self):
        self.assertIn("never answered", read_cycle([START, SPINNER]).verdict())

    def test_no_map_frame_after_the_answer_is_the_map_that_stopped(self):
        """**The failure this soak exists for**: the arm came back and nothing redrew."""
        self.assertIn("never caught up", read_cycle([START, SPINNER, ANSWER]).verdict())

    def test_an_arena_refusal_fails_the_cycle_however_it_ends(self):
        """A refused claim degrades silently on a shipping build, so the log line is the only
        witness."""
        self.assertIn("arena refused", read_cycle([START, BUSY, ANSWER, MAP]).verdict())

    def test_the_banner_is_one_repaint_per_freeze_not_one_per_pass(self):
        """The level→edge converter's whole point. Hundreds of ride-loop passes, one repaint — so a
        banner that *is* observed must be observed once."""
        self.assertIsNone(read_cycle([START, BANNER, ANSWER, MAP]).verdict())
        self.assertIn("repainting per pass", read_cycle([START, BANNER, BANNER, ANSWER, MAP]).verdict())

    def test_a_map_frame_while_the_arm_is_out_means_the_arena_was_not_exclusive(self):
        """Counts alone pass this — a start, an answer, and a catch-up repaint after it, all
        present. The *order* is what says a second map frame drew while the search still held the
        arm, which is the `render ⊥ nav` violation the soak is for."""
        torn = [START, SPINNER, MAP, ANSWER, MAP]
        self.assertIsNone(read_cycle(torn).verdict(), "counts see nothing wrong")
        self.assertIn("not exclusive", assert_sequence(torn))

    def test_a_banner_after_the_catch_up_means_the_freeze_outlived_its_search(self):
        self.assertIn("outlived", assert_sequence([START, ANSWER, MAP, BANNER]))


class Liveness(unittest.TestCase):
    def test_a_wedged_vcom_still_grows_the_log(self):
        """**The trap the probe exists for.** Counting bytes passes here; counting the board's own
        acknowledgements does not."""
        self.assertEqual(step_acks(WEDGED), 0)

    def test_acknowledgements_are_counted_not_merely_detected(self):
        """A lossy cable is worth reporting and is not a wedge, so the probe needs the count."""
        self.assertEqual(step_acks([STEP, MAP, STEP, STEP]), 3)


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
        self.assertEqual(faults(HEALTHY), [])


if __name__ == "__main__":
    unittest.main()
