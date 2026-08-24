#!/usr/bin/env python3
"""Drive the three `CoreMode` on-glass soaks (#1397 S5, #1487) over the DK's VCOM harness.

**Death trigger: delete this file when #1487 closes with soaks A, B and C recorded.** It exists to
produce one piece of evidence a host test cannot: on a shipping build a refused arena claim degrades
*silently* — the frame skips its map redraw and tries again — so "the map never redraws again" is a
failure only a running board can show. Once that run is recorded, the mechanism is covered by
`CoreMode`'s host tests and git history is the archive for this rig.

    # one shell: the RTT log this script reads
    cd firmware/obc-fw-nrf54l
    DEFMT_LOG=debug cargo rtt --release --features debug-uart | tee /tmp/s5-rtt.log

    # another: the driver
    python3 tools/s5_core_mode_soak.py A --rtt-log /tmp/s5-rtt.log --cycles 50
    python3 tools/s5_core_mode_soak.py B --rtt-log /tmp/s5-rtt.log
    python3 tools/s5_core_mode_soak.py C --rtt-log /tmp/s5-rtt.log --minutes 60

## Which flow the scenarios drive, and why it is `N` and not the Detour menu

Every plan below is posted with the debug link's `N <from_lon> <from_lat> <to_lon> <to_lat>` line
(`obc-platform/src/debug_link.rs`, consumed by `ride.rs`'s `debug_start_nav` arm), which is a real
`PlanRoute`. **The Detour menu cannot drive this soak**: the board has no detour half yet and answers
`DetourPlanned(Err(NoPath))` the moment the command drains (`ride.rs`'s `PlanDetour` arm), so
`nav_take_arena` is never called, `nav_begin` never runs, and neither `nav plan: start` nor any arena
claim ever happens. A detour-driven run reports failure on a perfectly healthy board.

`N` is the one flow that actually arms the planner, claims the arena's nav arm for the whole search,
and gives it back — which is the `render ⊥ nav` cycle these soaks exist to stress.

## What is *not* automatable here, stated rather than discovered

**The banner's on-glass appearance is not reachable by any gesture.** The freeze needs a live search
*and* a map base, and on the board the only gesture that puts a map base back under a running search
— Back on the planning screen — also posts the cancellation, which the ride loop drains **in the same
pass**: gestures are taken at the top of the loop body and `drain_host_commands` runs below them,
both before the render. `obc-app`'s own `the_board_loop_renders_the_map_again_the_pass_a_cancel_lands`
pins that ordering, and `App::debug_set_plan_live` exists precisely because of it.

So [`Cycle`] **counts** banner repaints and fails on more than one (a level repainting per pass is a
real regression), but does not require one. The banner's pixels are proven off-device by
`obc-sim --freeze --png`, and its legibility on the reflective panel is the human check #1487 already
flags. If a board detour half or a deferred drain ever makes the window real, the
`freeze: banner repaint` line is there and the ordering check picks it up.

## The rig, and the two ways it lies to you

* Build `--release --features debug-uart` — the sensors are swapped for the VCOM feed so a ride can
  be driven headlessly. **HWFC must be OFF** in the Board Configurator or host→device injection is
  silently ignored; `stty` + `printf` does not work, which is why this is pyserial at 115200 with
  `rtscts=False`.
* **The J-Link CDC wedges silently**: `write()` succeeds, RTT keeps flowing, and nothing lands — a
  blind script then "passes" every step against a board that heard nothing. [`liveness_probe`] runs
  before every scenario and between scenario A's cycles: send six taps and require the board's own
  `input: Step` acknowledgements back. A *grown log* is not enough — sensors and frames keep logging
  right through a wedge. Zero acknowledgements means wedged, and only a physical DK power-cycle
  clears it.
* `nav plan: start` is a `defmt::debug!`, so the RTT shell needs `DEFMT_LOG=debug`. Without it every
  cycle reports a missing start line and the run is worthless.

Everything above `Link` is pure log analysis with no pyserial in it, which is what
`tools/tests/test_s5_core_mode_soak.py` drives against recorded RTT text.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import glob
from pathlib import Path
import sys
import time

# ── the RTT vocabulary this soak reads (mirrors `firmware/obc-fw-nrf54l/src/ride.rs`) ────────────

PLAN_START = "nav plan: start"
PLAN_ANSWER = "nav route:"
PLAN_REFUSED = "nav: cannot start a plan"
MAP_FRAME = "map frame:"
# The frozen-overlay repaint's **own** line. Deliberately not `ui frame:` — the menus, the station
# steps and the planning spinner all take that same non-map branch, so counting `ui frame:` as a
# banner reports three banners for one healthy cycle and masks the ordering check behind it.
BANNER = "freeze: banner repaint"
USB_GRANTED = "arena: 64 KiB USB write-combining arm granted"
USB_RECLAIMED = "arena: USB write-combining arm reclaimed"
ARENA_REFUSED = "claim refused"
ARENA_RELEASE_REFUSED = "release refused"
STACK_PEAK = "stack high-water"
BOOT_FAULT = "boot fault"
# The board's own acknowledgement of an injected selection step (`ride.rs`'s input log). The liveness
# probe requires *this*, not merely a log that grew: RTT keeps flowing from sensors and frames while
# VCOM input is wedged, so growth alone passes a probe against a board that heard nothing.
STEP_ACK = "input: Step"

# The refusal string `nav_take_arena` answers a live cable transfer with. Scenario B's whole point:
# the rider must be told about the cable, not about "the scratch arena".
REFUSAL_TRANSFER = "a cable transfer holds the store"
REFUSAL_ARENA = "the scratch arena is busy"

# `stack_reserve` / `deep_ride_margin_min` from `firmware/tools/resource_baseline.json` — scenario C
# fails if a reported stack peak eats into the margin.
STACK_RESERVE = 65_536
DEEP_RIDE_MARGIN_MIN = 8_704


@dataclass
class Cycle:
    """What one `N` plan cycle produced, as read back out of the RTT log."""

    started: bool = False
    answered: bool = False
    outcome: str = ""
    banner_frames: int = 0
    map_frames: int = 0
    arena_refusals: list[str] = field(default_factory=list)
    refusals: list[str] = field(default_factory=list)

    def verdict(self) -> str | None:
        """`None` when the cycle passed, else why it did not."""
        if not self.started:
            if self.refusals:
                return f"the plan was refused: {self.refusals[0]}"
            return f"no `{PLAN_START}` line — the plan never armed (or DEFMT_LOG is not debug)"
        if not self.answered:
            return "the search never answered — a spinner that never resolves holds the nav arm forever"
        if self.arena_refusals:
            return f"the arena refused a claim: {'; '.join(self.arena_refusals)}"
        if self.map_frames == 0:
            # **The regression this whole soak exists for.** A refused claim degrades silently: the
            # frame skips its map redraw and tries again, so the only witness is a map that stops.
            return "no map frame after the answer — the arm came back but the map never caught up"
        if self.banner_frames > 1:
            return f"{self.banner_frames} banner repaints for one freeze — the edge is repainting per pass"
        return None


def read_cycle(lines: list[str]) -> Cycle:
    """Fold one cycle's RTT lines into a [`Cycle`]. Ordering is not asserted here — `verdict` is
    about counts, and `assert_sequence` below is what pins the order."""
    cycle = Cycle()
    for line in lines:
        if PLAN_START in line:
            cycle.started = True
        elif PLAN_ANSWER in line:
            cycle.answered = True
            tail = line.split(PLAN_ANSWER, 1)[1].split()
            cycle.outcome = tail[0] if tail else ""
        elif BANNER in line:
            cycle.banner_frames += 1
        elif MAP_FRAME in line and cycle.answered:
            cycle.map_frames += 1
        elif ARENA_REFUSED in line or ARENA_RELEASE_REFUSED in line:
            cycle.arena_refusals.append(line.strip())
        elif PLAN_REFUSED in line:
            cycle.refusals.append(line.strip())
    return cycle


def assert_sequence(lines: list[str]) -> str | None:
    """The order a cycle must walk: start → answer → the map catches up, with any banner repaint
    strictly **between the start and the answer**.

    The answer is where the freeze ends, not the catch-up: `nav_finish` logs `nav route:` and hands
    the app the answer in the same pass, `note_answer` clears the search level there, and the render
    decision comes after both. So a banner repaint anywhere at or past the answer says the level
    outlived the run that owned it — which is the stuck freeze, one pass early.

    Counts alone pass a transcript where a map frame landed *before* the answer, which is the other
    regression: the map plane drawing while the nav arm is still out."""
    order = [
        kind
        for line in lines
        for kind in (
            ["start"]
            if PLAN_START in line
            else ["answer"]
            if PLAN_ANSWER in line
            else ["banner"]
            if BANNER in line
            else ["map"]
            if MAP_FRAME in line
            else []
        )
    ]
    if "start" not in order:
        return "no plan start in this window"
    after = order[order.index("start") :]
    if "answer" not in after:
        return "no plan answer after the start"
    answer_at = after.index("answer")
    if "map" in after[:answer_at]:
        return "a full map repaint landed while the search still held the arm — the arena was not exclusive"
    if "map" not in after[answer_at:]:
        return "no map frame after the answer — the map did not catch up"
    if "banner" in after[answer_at:]:
        return "a banner repaint at or after the answer — the freeze outlived its search"
    return None


def stack_peaks(lines: list[str]) -> list[int]:
    """Every `stack high-water N / M B` peak in the window, in bytes."""
    peaks = []
    for line in lines:
        if STACK_PEAK not in line:
            continue
        tail = line.split(STACK_PEAK, 1)[1].split("/")[0]
        digits = "".join(c for c in tail if c.isdigit())
        if digits:
            peaks.append(int(digits))
    return peaks


def margin_verdict(peaks: list[int]) -> str | None:
    """Scenario C's stack gate: the deepest reported peak must leave `deep_ride_margin_min` free."""
    if not peaks:
        return None
    worst = max(peaks)
    margin = STACK_RESERVE - worst
    if margin < DEEP_RIDE_MARGIN_MIN:
        return f"stack peak {worst} B leaves {margin} B, under the {DEEP_RIDE_MARGIN_MIN} B floor"
    return None


def step_acks(lines: list[str]) -> int:
    """How many injected selection steps the board acknowledged in this window."""
    return sum(STEP_ACK in line for line in lines)


def faults(lines: list[str]) -> list[str]:
    """Boot faults and WDT resets seen in the window — either one ends a soak."""
    return [line.strip() for line in lines if BOOT_FAULT in line.lower() or "watchdog" in line.lower()]


# ── the wire ─────────────────────────────────────────────────────────────────────────────────────


class Link:
    """The VCOM line, and the RTT log beside it. Every `send` is one `\\n`-terminated debug-link
    message (`obc_platform::debug_link`'s wire)."""

    def __init__(self, port: str, rtt_log: Path, baud: int = 115_200) -> None:
        import serial  # imported here so the analysis half above stays importable without pyserial

        self.port = serial.Serial(port, baud, timeout=1, rtscts=False)
        self.rtt_log = rtt_log

    def send(self, line: str) -> None:
        self.port.write((line + "\n").encode())
        self.port.flush()

    def mark(self) -> int:
        """The RTT log's current size — the cursor `since` reads from."""
        return self.rtt_log.stat().st_size if self.rtt_log.exists() else 0

    def since(self, mark: int) -> list[str]:
        """Every RTT line written since `mark`."""
        if not self.rtt_log.exists():
            return []
        with self.rtt_log.open("r", errors="replace") as fh:
            fh.seek(mark)
            return fh.readlines()

    def wait_for(self, mark: int, needle: str, timeout: float) -> bool:
        """Poll the log until `needle` appears after `mark`, or give up. The plan phases run for
        hundreds of ms to seconds, so every step waits on its own landmark rather than on a sleep
        long enough to cover the worst case."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if any(needle in line for line in self.since(mark)):
                return True
            time.sleep(0.1)
        return False

    # -- the gestures and commands, in the debug-link vocabulary --

    def press(self) -> None:
        self.send("K s d")
        time.sleep(0.05)
        self.send("K s u")

    def back(self) -> None:
        self.send("K b d")
        time.sleep(0.05)
        self.send("K b u")

    def step(self, n: int) -> None:
        self.send(f"K t {n}")

    def fix(self, lat_ud: int, lon_ud: int) -> None:
        self.send(f"F {lat_ud} {lon_ud}")

    def zoom(self, mpp: float) -> None:
        self.send(f"Z {mpp}")

    def plan(self, frm: tuple[int, int], to: tuple[int, int]) -> None:
        """`N <from_lon> <from_lat> <to_lon> <to_lat>` — **LON FIRST**, unlike the lat-first `F`."""
        self.send(f"N {frm[0]} {frm[1]} {to[0]} {to[1]}")


def liveness_probe(link: Link) -> None:
    """**Run this before trusting a single assertion.** The J-Link CDC wedges with `write()` still
    succeeding and RTT still flowing, and a blind script then passes every step against a board that
    heard nothing.

    Six taps must come back as the board's own `input: Step` acknowledgements. A grown log is *not*
    enough — sensors and frames keep logging through a wedge. Zero acknowledgements is a wedge, and
    only a physical DK power-cycle clears it; fewer than six is a lossy cable, worth saying out loud
    but not worth sending someone to the power switch."""
    mark = link.mark()
    for _ in range(6):
        link.step(1)
        time.sleep(0.1)
    time.sleep(2.0)
    acks = step_acks(link.since(mark))
    if acks == 0:
        raise SystemExit(
            "VCOM is wedged: six injected taps produced no `input: Step` acknowledgement.\n"
            "Power-cycle the DK physically (a re-flash does not clear it) and start again."
        )
    if acks < 6:
        print(f"  liveness: only {acks}/6 taps acknowledged — the cable is lossy, results may be noisy")


# ── the scenarios ────────────────────────────────────────────────────────────────────────────────


def ride_to_map(link: Link) -> None:
    """Fresh boot → Home → RouteMenu → RouteOverview → Map; the ride starts."""
    for _ in range(3):
        link.press()
        time.sleep(0.4)


def stream_fixes(link: Link, base: tuple[int, int], steps: int, delay: float = 1.0) -> None:
    """`F` fixes at ~1 Hz in small increments — teleport rejection drops anything larger."""
    lat, lon = base
    for i in range(steps):
        link.fix(lat + i * 40, lon + i * 40)
        time.sleep(delay)


def plan_cycle(
    link: Link, frm: tuple[int, int], to: tuple[int, int], base: tuple[int, int]
) -> tuple[Cycle, list[str]]:
    """One whole `N` plan: post it, wait for the arm, let it search under a live ride, wait for the
    answer, then Back off the result screen so the map base is what catches up.

    The Back is exactly one press: `land_route_plan` **replaces** the `NavPlanning` entry in place
    (with `RouteOverview` on success, `NavFail` on either failure tier), so the stack is
    `[…, Map, <result>]` and one Back leaves the Map on top."""
    mark = link.mark()
    link.plan(frm, to)
    if link.wait_for(mark, PLAN_START, timeout=4.0):
        # A live ride under the search: fixes keep landing (a freeze pauses the map, never the ride)
        # and the planner steps between frames.
        stream_fixes(link, base, 3)
        link.wait_for(mark, PLAN_ANSWER, timeout=20.0)
    link.back()  # off the result screen, back onto the map base
    time.sleep(0.3)
    stream_fixes(link, base, 3)
    lines = link.since(mark)
    return read_cycle(lines), lines


def report(prefix: str, cycle: Cycle, lines: list[str]) -> int:
    """Print a cycle's verdict; return 1 when it failed. A failure prints its landmark tally, because
    the usual cause is the key sequence drifting, not the firmware."""
    why = cycle.verdict() or assert_sequence(lines)
    if not why:
        return 0
    print(f"  {prefix}: FAIL — {why}")
    print(
        f"    landmarks: start={cycle.started} answer={cycle.outcome or '-'} "
        f"banner={cycle.banner_frames} map={cycle.map_frames} "
        f"arena_refusals={len(cycle.arena_refusals)} plan_refusals={len(cycle.refusals)}"
    )
    return 1


def scenario_a(link: Link, cycles: int, frm: tuple[int, int], to: tuple[int, int], base: tuple[int, int]) -> int:
    """**A — the `render ⊥ nav` claim/release cycle**, ≥50 times.

    Each cycle must arm the planner, answer, give the arm back and let the map catch up, with zero
    arena refusals in between. The failure it hunts is a map that stops redrawing — silent on a
    shipping build, which is why it is only visible here."""
    print(f"A: {cycles} plan cycles (render ⊥ nav claim/release)")
    ride_to_map(link)
    stream_fixes(link, base, 3)
    failures = 0
    banners = 0
    for i in range(cycles):
        if i % 10 == 0:
            liveness_probe(link)
        cycle, lines = plan_cycle(link, frm, to, (base[0] + i, base[1] + i))
        banners += cycle.banner_frames
        failed = report(f"cycle {i}", cycle, lines)
        failures += failed
        if i % 10 == 0 and not failed:
            print(f"  cycle {i}: ok ({cycle.outcome})")
    print(f"A: {cycles - failures}/{cycles} cycles passed; {banners} banner repaints observed")
    print("   (0 banner repaints is expected — see the module docs on why no gesture engages the freeze)")
    return failures


def scenario_b(link: Link, frm: tuple[int, int], to: tuple[int, int]) -> int:
    """**B — `nav ⊥ usb` and `render ⊥ usb`.** Semi-automatic: a hand on the cable.

    The plan offered during an upload must be refused *before* the planner arms, and the refusal must
    name the transfer — not "the scratch arena is busy", and not a `NoPath`.

    **The precondition, and it is the whole basis of the claim:** the refusal is reachable only while
    the USB **write-combining arm** is actually held, which the ride loop takes on
    `usb::stage_requested()` and announces as `arena: 64 KiB USB write-combining arm granted`. Without
    that grant the arena is free and the plan proceeds beside the upload exactly as it did before this
    slice — so the step below checks for the grant first and says so rather than failing the board."""
    # Every mark around an `input()` is taken **before** the prompt. Both arena transitions are
    # edges the board logs once, in the same pass as the card they accompany — and the operator is
    # slower than a pass, so a mark taken after they press Enter starts the window past the very
    # line it is waiting for. That is a healthy board reported as a SKIP (or as a false refusal).
    mark = link.mark()
    print("B: plug the cable into J3, load a route, and start a map upload; press Enter when the")
    print("   transfer card is up.")
    input()
    if not link.wait_for(mark, USB_GRANTED, timeout=10.0):
        print("  SKIP — the USB write-combining arm was never granted, so the arena is free and this")
        print("         step tests nothing. Re-run with an upload large enough to request it.")
        return 0

    mark = link.mark()
    link.plan(frm, to)
    time.sleep(2.0)
    lines = link.since(mark)
    refusals = [line.strip() for line in lines if PLAN_REFUSED in line]
    failures = 0
    if any(PLAN_START in line for line in lines):
        print("  FAIL — the planner armed during the upload; `nav ⊥ usb` did not hold")
        failures += 1
    elif not refusals:
        print("  FAIL — the plan was neither armed nor refused")
        failures += 1
    elif any(REFUSAL_ARENA in line for line in refusals):
        print(f"  FAIL — the rider was told the arena is busy; the cable is the actionable fact: {refusals[0]}")
        failures += 1
    elif not any(REFUSAL_TRANSFER in line for line in refusals):
        print(f"  FAIL — refused, but not by name: {refusals[0]}")
        failures += 1
    else:
        print("  refused before the planner armed, and the refusal names the transfer: ok")

    reclaim_mark = link.mark()
    print("B: end the upload, then press Enter.")
    input()
    # The arm has to be *given back* before the plan can be expected to arm. Without this wait the
    # next `N` races a guard that is still held, and the run reports a false refusal — the same
    # mistake in the other direction. An arm that never comes back is the stuck-USB mirror of the
    # stuck-nav-arm bug this whole soak hunts, so it fails rather than waiting forever.
    if not link.wait_for(reclaim_mark, USB_RECLAIMED, timeout=10.0):
        print("  FAIL — the USB write-combining arm was never reclaimed after the upload ended")
        failures += 1
    else:
        mark = link.mark()
        link.plan(frm, to)
        if not link.wait_for(mark, PLAN_START, timeout=5.0):
            print("  FAIL — no plan armed after the upload ended")
            failures += 1
        elif any(PLAN_REFUSED in line for line in link.since(mark)):
            print("  FAIL — the same input is still refused after the upload ended")
            failures += 1
        else:
            print("  the plan arms normally afterwards: ok")
            link.wait_for(mark, PLAN_ANSWER, timeout=20.0)
            link.back()

    whole = link.since(0)
    granted = sum(USB_GRANTED in line for line in whole)
    reclaimed = sum(USB_RECLAIMED in line for line in whole)
    print(f"  arena arm: {granted} granted / {reclaimed} reclaimed (expect one each per upload)")
    return failures


def scenario_c(link: Link, minutes: int, frm: tuple[int, int], to: tuple[int, int], base: tuple[int, int]) -> int:
    """**C — the stuck-arm soak.** A continuous ride with a plan cycle every ~60 s and a periodic
    zoom nudge. Every released nav arm must be followed by a map render, and the stack must never eat
    into `deep_ride_margin_min`."""
    print(f"C: {minutes} minutes of continuous riding with a plan cycle every ~60 s")
    ride_to_map(link)
    deadline = time.time() + minutes * 60
    failures = 0
    n = 0
    while time.time() < deadline:
        mark = link.mark()
        link.zoom(30.0 if n % 2 else 20.0)
        stream_fixes(link, (base[0] + n * 10, base[1]), 25)
        cycle, lines = plan_cycle(link, frm, to, (base[0] + n * 10, base[1]))
        failures += report(f"cycle {n}", cycle, lines)
        window = link.since(mark)
        for problem in [margin_verdict(stack_peaks(window))] + faults(window):
            if problem:
                failures += 1
                print(f"  cycle {n}: FAIL — {problem}")
        n += 1
    print(f"C: {n} cycles over {minutes} min, {failures} failures")
    return failures


def coord(raw: str) -> tuple[int, int]:
    """`LON,LAT` in integer microdegrees — the `N` line's own order."""
    lon, lat = raw.split(",")
    return int(lon), int(lat)


def resolve_port(explicit: str | None) -> str:
    if explicit:
        return explicit
    matches = sorted(glob.glob("/dev/cu.usbmodem*133"))
    if not matches:
        raise SystemExit("no /dev/cu.usbmodem*133 — is the DK plugged in?")
    return matches[0]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("scenario", choices=["A", "B", "C"])
    parser.add_argument("--port", help="VCOM device (default: the first /dev/cu.usbmodem*133)")
    parser.add_argument("--rtt-log", required=True, type=Path, help="the file `cargo rtt` is tee'd into")
    parser.add_argument("--cycles", type=int, default=50, help="scenario A plan cycles (>=50 is the gate)")
    parser.add_argument("--minutes", type=int, default=60, help="scenario C duration (60 is the gate)")
    # Grimsel defaults, on the map the fixture registry ships. Both ends must lie on the mounted
    # map's routing graph or every cycle answers `no-path` — which the run reports as its outcome.
    parser.add_argument("--nav-from", type=coord, default=(8_337_000, 46_562_000), help="LON,LAT µdeg")
    parser.add_argument("--nav-to", type=coord, default=(8_248_000, 46_570_000), help="LON,LAT µdeg")
    parser.add_argument("--lat", type=int, default=46_562_000, help="streamed fix latitude, µdeg")
    parser.add_argument("--lon", type=int, default=8_337_000, help="streamed fix longitude, µdeg")
    args = parser.parse_args()

    if not args.rtt_log.exists():
        raise SystemExit(f"{args.rtt_log} does not exist — start `DEFMT_LOG=debug cargo rtt … | tee` first")

    link = Link(resolve_port(args.port), args.rtt_log)
    liveness_probe(link)
    base = (args.lat, args.lon)
    if args.scenario == "A":
        failures = scenario_a(link, args.cycles, args.nav_from, args.nav_to, base)
    elif args.scenario == "B":
        failures = scenario_b(link, args.nav_from, args.nav_to)
    else:
        failures = scenario_c(link, args.minutes, args.nav_from, args.nav_to, base)

    print("PASS" if failures == 0 else f"FAIL ({failures})")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
