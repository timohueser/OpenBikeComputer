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

## The rig, and the two ways it lies to you

* Build `--release --features debug-uart` — the sensors are swapped for the VCOM feed so a ride can
  be driven headlessly. **HWFC must be OFF** in the Board Configurator or host→device injection is
  silently ignored; `stty` + `printf` does not work, which is why this is pyserial at 115200 with
  `rtscts=False`.
* **The J-Link CDC wedges silently**: `write()` succeeds, RTT keeps flowing, and nothing lands — a
  blind script then "passes" every step. [`liveness_probe`] runs before every scenario and between
  scenario A's cycles: snapshot the RTT log size, send six taps, wait, re-check. Zero growth means
  wedged, and only a physical DK power-cycle clears it.
* `nav plan: start` is a `defmt::debug!`, so the RTT shell needs `DEFMT_LOG=debug`. Without it every
  cycle reports a missing start line and the run is worthless.

Everything above `Link` is pure log/plan analysis with no pyserial in it, which is what
`tools/tests/test_s5_core_mode_soak.py` drives against recorded RTT text.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import glob
import os
from pathlib import Path
import sys
import time

# ── the RTT vocabulary this soak reads (mirrors `firmware/obc-fw-nrf54l/src/ride.rs`) ────────────

PLAN_START = "nav plan: start"
MAP_FRAME = "map frame:"
UI_FRAME = "ui frame:"
PLAN_REFUSED = "nav: cannot start a plan"
USB_GRANTED = "arena: 64 KiB USB write-combining arm granted"
USB_RECLAIMED = "arena: USB write-combining arm reclaimed"
ARENA_REFUSED = "claim refused"
ARENA_RELEASE_REFUSED = "release refused"
STACK_PEAK = "stack high-water"
BOOT_FAULT = "boot fault"

# The refusal string `nav_take_arena` answers a live cable transfer with. Scenario B's whole point:
# the rider must be told about the cable, not about "the scratch arena".
REFUSAL_TRANSFER = "a cable transfer holds the store"
REFUSAL_ARENA = "the scratch arena is busy"

# `deep_ride_margin_min` from `firmware/tools/resource_baseline.json` — scenario C fails if a
# reported stack peak eats into it.
STACK_RESERVE = 65_536
DEEP_RIDE_MARGIN_MIN = 8_704


@dataclass
class Cycle:
    """What one plan cycle produced, as read back out of the RTT log."""

    started: bool = False
    banner_frames: int = 0
    map_frames: int = 0
    arena_refusals: list[str] = field(default_factory=list)
    refusals: list[str] = field(default_factory=list)

    def verdict(self) -> str | None:
        """`None` when the cycle passed, else why it did not."""
        if not self.started:
            return f"no `{PLAN_START}` line — the plan never armed (or DEFMT_LOG is not debug)"
        if self.banner_frames == 0:
            return "the freeze raised no banner frame — the rider saw a map that simply stopped"
        if self.banner_frames > 1:
            return f"{self.banner_frames} banner frames for one freeze — the edge is repainting per pass"
        if self.map_frames != 1:
            return f"{self.map_frames} full map repaints after the answer — expected exactly one catch-up"
        if self.arena_refusals:
            return f"the arena refused a claim: {'; '.join(self.arena_refusals)}"
        return None


def read_cycle(lines: list[str]) -> Cycle:
    """Fold one cycle's RTT lines into a [`Cycle`]. Ordering is not asserted here — `verdict` is
    about counts, and `assert_sequence` below is what pins the order."""
    cycle = Cycle()
    for line in lines:
        if PLAN_START in line:
            cycle.started = True
        elif UI_FRAME in line and cycle.started:
            cycle.banner_frames += 1
        elif MAP_FRAME in line and cycle.started:
            cycle.map_frames += 1
        elif ARENA_REFUSED in line or ARENA_RELEASE_REFUSED in line:
            cycle.arena_refusals.append(line.strip())
        elif PLAN_REFUSED in line:
            cycle.refusals.append(line.strip())
    return cycle


def assert_sequence(lines: list[str]) -> str | None:
    """The order scenario A pins: start → banner → the catch-up repaint. A map frame *before* the
    banner means the map plane drew while the nav arm was out, which is the whole regression."""
    order = [
        kind
        for line in lines
        for kind in (
            ["start"]
            if PLAN_START in line
            else ["banner"]
            if UI_FRAME in line
            else ["map"]
            if MAP_FRAME in line
            else []
        )
    ]
    if "start" not in order:
        return "no plan start in this window"
    after = order[order.index("start") :]
    if "banner" not in after:
        return "no banner after the start"
    if "map" in after and after.index("map") < after.index("banner"):
        return "a full map repaint landed between the start and the banner — the freeze did not hold"
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

    # -- the gestures, in the debug-link vocabulary --

    def press(self) -> None:
        self.send("K s d")
        time.sleep(0.05)
        self.send("K s u")

    def back(self) -> None:
        self.send("K b d")
        time.sleep(0.05)
        self.send("K b u")

    def back_hold(self, ms: int = 900) -> None:
        self.send("K b d")
        time.sleep(ms / 1000)
        self.send("K b u")

    def step(self, n: int) -> None:
        self.send(f"K t {n}")

    def fix(self, lat_ud: int, lon_ud: int) -> None:
        self.send(f"F {lat_ud} {lon_ud}")

    def zoom(self, mpp: float) -> None:
        self.send(f"Z {mpp}")


def liveness_probe(link: Link) -> None:
    """**Run this before trusting a single assertion.** The J-Link CDC wedges with `write()` still
    succeeding and RTT still flowing, and a blind script then passes every step against a board that
    heard nothing. Six taps must move the RTT log; zero growth is a wedge, and only a physical DK
    power-cycle clears it."""
    mark = link.mark()
    for _ in range(6):
        link.step(1)
        time.sleep(0.1)
    time.sleep(2.0)
    if link.mark() == mark:
        raise SystemExit(
            "VCOM is wedged: six injected taps produced no RTT output at all.\n"
            "Power-cycle the DK physically (a re-flash does not clear it) and start again."
        )


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


def scenario_a(link: Link, cycles: int, base: tuple[int, int]) -> int:
    """**A — the freeze window over a map base (`render ⊥ nav`).**

    The only way a map base lands under a live search: back out of the planning spinner while the
    planner is still running. Each cycle must show start → banner → answer → exactly one full
    repaint, with no arena refusal anywhere."""
    print(f"A: {cycles} freeze cycles over a map base")
    ride_to_map(link)
    stream_fixes(link, base, 3)
    failures = 0
    for i in range(cycles):
        if i % 10 == 0:
            liveness_probe(link)
        mark = link.mark()
        link.back_hold()  # the ride menu
        time.sleep(0.4)
        link.step(1)  # → Detour
        time.sleep(0.2)
        link.press()  # the rejoin chooser
        time.sleep(0.4)
        link.press()  # posts the plan and pushes the spinner
        time.sleep(0.15)
        link.back()  # …and pop it *while the planner runs* — THE window
        stream_fixes(link, (base[0] + i, base[1] + i), 3)
        lines = link.since(mark)
        cycle = read_cycle(lines)
        why = cycle.verdict() or assert_sequence(lines)
        if why:
            failures += 1
            print(f"  cycle {i}: FAIL — {why}")
        elif i % 10 == 0:
            print(f"  cycle {i}: ok")
    print(f"A: {cycles - failures}/{cycles} cycles passed")
    return failures


def scenario_b(link: Link) -> int:
    """**B — `nav ⊥ usb` and `render ⊥ usb`.** Semi-automatic: a hand on the cable.

    The plan offered during an upload must be refused *before* the planner arms, and the refusal
    must name the transfer — not "the scratch arena is busy", and not a `NoPath`."""
    print("B: plug the cable into J3, load a route, and start a map upload; press Enter when the")
    print("   transfer card is up.")
    input()
    mark = link.mark()
    link.back_hold()
    time.sleep(0.4)
    link.step(1)
    time.sleep(0.2)
    link.press()
    time.sleep(0.4)
    link.press()
    time.sleep(1.0)
    lines = link.since(mark)
    refusals = [line for line in lines if PLAN_REFUSED in line]
    failures = 0
    if not refusals:
        print("  FAIL — the plan was not refused during the upload")
        failures += 1
    elif not any(REFUSAL_TRANSFER in line for line in refusals):
        print(f"  FAIL — refused, but not by name: {refusals[0].strip()}")
        failures += 1
    elif any(REFUSAL_ARENA in line for line in refusals):
        print("  FAIL — the rider was told the arena is busy; the cable is the actionable fact")
        failures += 1
    else:
        print("  refusal names the transfer: ok")

    print("B: end the upload, then press Enter.")
    input()
    mark = link.mark()
    link.back_hold()
    time.sleep(0.4)
    link.step(1)
    time.sleep(0.2)
    link.press()
    time.sleep(0.4)
    link.press()
    time.sleep(1.0)
    lines = link.since(mark)
    if any(PLAN_REFUSED in line for line in lines):
        print("  FAIL — the same input is still refused after the upload ended")
        failures += 1
    elif not any(PLAN_START in line for line in lines):
        print("  FAIL — no plan started after the upload ended")
        failures += 1
    else:
        print("  the plan arms normally afterwards: ok")

    grants = sum(1 for line in lines if USB_GRANTED in line)
    reclaims = sum(1 for line in lines if USB_RECLAIMED in line)
    print(f"  arena arm: {grants} granted / {reclaims} reclaimed (expect one each per upload)")
    return failures


def scenario_c(link: Link, minutes: int, base: tuple[int, int]) -> int:
    """**C — the stuck-freeze soak.** A continuous ride with a plan cycle every ~60 s and a periodic
    zoom nudge. Every freeze release must be followed by a map render within two wakes, and the
    stack must never eat into `deep_ride_margin_min`."""
    print(f"C: {minutes} minutes of continuous riding with a plan cycle every 60 s")
    ride_to_map(link)
    deadline = time.time() + minutes * 60
    failures = 0
    cycle_no = 0
    while time.time() < deadline:
        mark = link.mark()
        link.zoom(30.0 if cycle_no % 2 else 20.0)
        stream_fixes(link, (base[0] + cycle_no * 10, base[1]), 25)
        link.back_hold()
        time.sleep(0.4)
        link.step(1)
        time.sleep(0.2)
        link.press()
        time.sleep(0.4)
        link.press()
        time.sleep(0.15)
        link.back()
        stream_fixes(link, (base[0] + cycle_no * 10, base[1]), 20)
        lines = link.since(mark)
        why = read_cycle(lines).verdict()
        fault_lines = faults(lines)
        margin = margin_verdict(stack_peaks(lines))
        for problem in [why, margin] + fault_lines:
            if problem:
                failures += 1
                print(f"  cycle {cycle_no}: FAIL — {problem}")
        cycle_no += 1
    print(f"C: {cycle_no} cycles over {minutes} min, {failures} failures")
    return failures


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
    parser.add_argument("--lat", type=int, default=47_990_000, help="starting fix latitude, microdegrees")
    parser.add_argument("--lon", type=int, default=7_850_000, help="starting fix longitude, microdegrees")
    args = parser.parse_args()

    if not args.rtt_log.exists():
        raise SystemExit(f"{args.rtt_log} does not exist — start `DEFMT_LOG=debug cargo rtt … | tee` first")

    link = Link(resolve_port(args.port), args.rtt_log)
    liveness_probe(link)
    base = (args.lat, args.lon)
    if args.scenario == "A":
        failures = scenario_a(link, args.cycles, base)
    elif args.scenario == "B":
        failures = scenario_b(link)
    else:
        failures = scenario_c(link, args.minutes, base)

    print("PASS" if failures == 0 else f"FAIL ({failures})")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
