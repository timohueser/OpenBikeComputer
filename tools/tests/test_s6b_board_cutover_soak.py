"""The board cutover soak driver's log analysis, against recorded RTT text.

**Death trigger: delete with `tools/s6b_board_cutover_soak.py` when #1494 closes.** The script's
whole value is that a blind driver "passes" everything — a wedged VCOM, a `DEFMT_LOG` that swallows
the plan-start line, a map frame that lands while the nav arm is still out, a delete that never
committed. These pin the verdicts that catch each of those, off recorded lines, so the rig is not
itself the thing under test on the board.

Every transcript below is **realistic**: it carries the `ui frame:` lines a real cycle emits for the
planning spinner and the menus. The S5 rig's own record includes a revision that keyed the banner on
`ui frame:` and so reported three banner repaints for a healthy cycle; the fixtures are noisy on
purpose so that cannot come back.
"""

import unittest

from tools.s6b_board_cutover_soak import (
    DEEP_RIDE_MARGIN_MIN,
    STACK_RESERVE,
    alarms,
    assert_sequence,
    catalog_reads,
    faults,
    margin_verdict,
    read_cycle,
    removals,
    stack_peaks,
    step_acks,
    transfer_edges,
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
WEDGED = [MAP, "0.3 INFO  gps: fix 47994000 7849000", SPINNER]
REFUSED = "0.120 WARN  nav: cannot start a plan (a cable transfer holds the store) — refusing the operation"

# The typed executor's own lines (#1494).
REMOVED_REAL = "1.100 INFO  catalog: object 42 removed (existed true)"
REMOVED_GONE = "1.100 INFO  catalog: object 42 removed (existed false)"
REMOVE_FAIL = "1.050 WARN  catalog: object 42 removal failed — the domain re-queues it"
READ = "1.200 INFO  flat: Route menu loaded 7 route(s)"
XFER_ON = "2.000 INFO  xfer: transfer level active (flat engine)"
XFER_OFF = "2.900 INFO  xfer: transfer level idle (flat engine)"
ALARM_EFFECT = "3.000 ERROR exec: an effect this board cannot serve was decided (recorder #1398 / weather #1401)"
ALARM_RESIDUAL = (
    "3.100 ERROR exec: DeleteRoute { id: 4 } came back on the legacy protocol — DeviceCore owns it "
    "now, so it is skipped"
)

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

    def test_a_banner_at_or_after_the_answer_means_the_freeze_outlived_its_search(self):
        """The banner belongs strictly between the start and the answer. Under the typed executor the
        freeze is released one pass *after* `nav route:` (the `PlanFinished` outcome is consumed by
        the next pass, which `RideExec::owed` asks for immediately) — so a banner at or past the
        answer is still the stuck freeze, caught one pass earlier."""
        self.assertIn("outlived", assert_sequence([START, ANSWER, BANNER, MAP]))
        self.assertIn("outlived", assert_sequence([START, ANSWER, MAP, BANNER]))
        self.assertIsNone(assert_sequence([START, BANNER, ANSWER, MAP]), "…and before it is the freeze")


class ExecutorAlarms(unittest.TestCase):
    """The typed executor's `defmt::error!`s are the *only* witness on a release image: the matching
    `debug_assert!` is compiled out, and every one of these shapes was impossible before the cutover
    because the drain performed the work itself."""

    def test_an_unservable_effect_fails_the_cycle_it_lands_in(self):
        cycle = read_cycle([START, ALARM_EFFECT, ANSWER, MAP])
        self.assertIn("raised an alarm", cycle.verdict())
        self.assertIn("cannot serve", cycle.verdict())

    def test_a_class_that_came_back_on_the_legacy_protocol_is_an_alarm(self):
        """The residual is three commands. A fourth reappearing means the migration came undone —
        and the board *skips* it rather than performing it twice, so nothing else would show."""
        self.assertEqual(len(alarms([SPINNER, ALARM_RESIDUAL, MAP])), 1)

    def test_a_healthy_cycle_raises_none(self):
        self.assertEqual(alarms(HEALTHY), [])


class TypedRemoval(unittest.TestCase):
    def test_a_real_object_and_an_absent_one_are_told_apart(self):
        """`existed false` is a **success** — the subject vanished before the commit and the goal
        state holds (#1433 §13). A rig that could not tell the two apart would pass the one shape
        that must not read as a failure and fail the one that must."""
        self.assertEqual(removals([REMOVED_REAL]), [True])
        self.assertEqual(removals([REMOVED_GONE]), [False])

    def test_one_hold_must_answer_exactly_once(self):
        """Two answers for one hold is a re-queued delete that committed twice — the failure the
        answering path exists to make impossible."""
        self.assertEqual(len(removals([REMOVED_REAL, SPINNER, REMOVED_REAL])), 2)

    def test_a_refused_commit_is_visible_beside_the_answer_that_follows_it(self):
        """The store may refuse and the domain re-queues; scenario E reports that as the design
        rather than as a failure, so both lines have to be readable from one window."""
        window = [REMOVE_FAIL, SPINNER, REMOVED_REAL]
        self.assertEqual(removals(window), [True])
        self.assertTrue(any("removal failed" in line for line in window))

    def test_a_map_frame_is_not_mistaken_for_a_removal(self):
        self.assertEqual(removals(HEALTHY), [])


class CatalogRefresh(unittest.TestCase):
    def test_one_commit_orders_one_re_read(self):
        """The whole point of reporting `FlatStore::sequence()` as a level instead of counting commit
        edges: the old seam turned N commits into N `StoreChanged` events and therefore N rescans."""
        self.assertEqual(catalog_reads([READ]), 1)
        self.assertEqual(catalog_reads([READ, SPINNER, READ]), 2)
        self.assertEqual(catalog_reads(HEALTHY), 0)


class TransferLevel(unittest.TestCase):
    def test_the_level_edges_are_read_in_order(self):
        """S5 open question 2, closed: a route upload moves the level now. Order matters — an
        `active` with no `idle` after it is a stuck transfer level, which withdraws heavy work for
        the rest of the boot."""
        self.assertEqual(transfer_edges([XFER_ON, MAP, XFER_OFF]), ["active", "idle"])
        self.assertEqual(transfer_edges([XFER_ON, MAP]), ["active"])
        self.assertEqual(transfer_edges(HEALTHY), [])


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
