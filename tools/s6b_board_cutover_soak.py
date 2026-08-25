#!/usr/bin/env python3
"""Drive the board's `App::run_pass` cutover soak (#1397 S6b, #1494) over the DK's VCOM harness.

**Death trigger: delete this file when #1494 closes with scenarios A–K recorded.** It exists to
produce evidence no host test can: this slice replaces the loop that owns the watchdog, the scratch
arena, the store guard and the present, the board crate has no automated coverage at all, and the rig
that proved the *previous* spine (`tools/s5_core_mode_soak.py`) was deleted with its purpose when
#1487 closed. Every property S5 proved is unproven on the new spine, which is why A/B/C are re-run
here rather than inherited. Once this run is recorded, `CoreMode`, the pass and the shared residual
list are covered by host tests and git history is the archive for this rig.

    # one shell: the RTT log this script reads
    cd firmware/obc-fw-nrf54l
    DEFMT_LOG=debug cargo rtt --release --features debug-uart | tee /tmp/s6b-rtt.log

    # another: the driver
    python3 tools/s6b_board_cutover_soak.py A --rtt-log /tmp/s6b-rtt.log --cycles 50
    python3 tools/s6b_board_cutover_soak.py C --rtt-log /tmp/s6b-rtt.log --minutes 60
    python3 tools/s6b_board_cutover_soak.py DEFHIJ --rtt-log /tmp/s6b-rtt.log   # the typed halves
    python3 tools/s6b_board_cutover_soak.py B --rtt-log /tmp/s6b-rtt.log        # hand on the cable
    python3 tools/s6b_board_cutover_soak.py G --rtt-log /tmp/s6b-rtt.log        # hand on the phone

## The five rig lessons the S5 record paid for, carried forward

1. **Single-write tap edges.** `K b d\\nK b u` in **one** `write()`. A 50 ms host-side gap between
   the down and the up edge reads as a *Hold* on a loaded board, which navigates somewhere else and
   then every later assertion is about the wrong screen.
2. **A per-cycle `map frame:` self-heal, with a loud abort.** A cycle that ends with no map base
   under it poisons every cycle after it. [`heal_to_map`] presses Back until a `map frame:` line
   lands, and aborts the run — loudly — if it cannot get back, rather than reporting 49 bogus
   failures.
3. **A CDC settle plus one liveness retry.** The J-Link CDC needs ~1 s after open before it carries
   anything, and the first probe after a fresh open legitimately loses taps. [`liveness_probe`]
   settles, probes, and retries **once** before declaring a wedge.
4. **Bounded polls, never fixed observation windows.** Every step waits on its own landmark with a
   timeout ([`Link.wait_for`]); a `sleep` long enough for the worst case makes a 50-cycle run take an
   hour and still races the slow path.
5. **In-map coordinates.** The Freiburg N–S axis (`7849000,47994000 → 7846000,48004000`) is on the
   fixture map this board actually mounts. The Grimsel defaults the S5 rig shipped are outside the
   mounted bbox, and every cycle then answers `no-path` — a healthy board reported as a failure.

## What this slice changed about the *shape* of a cycle, and what the rig therefore reads

The board no longer drains `HostCommand`s. One `App::run_pass` per frame decides everything and the
executor serves its bounded effects in the physical phase that already owned them, so several things
that used to land in one pass now take two — and the loop asks for that second pass immediately
(`RideExec::owed`), so it is milliseconds, not a wake. Concretely:

* a `PlanRoute` decided by pass *N* arms the planner at the top of pass *N+1*'s store phase;
* a delete decided by pass *N* becomes a `Request::RemoveObject` at *N+1*, is answered at *N+2*, and
  the catalog re-read the commit orders lands at *N+3*;
* a `DfuEffect` is served in the guard-free block **ahead of** the store phase, so it is served one
  frame after it is decided — and its outcome is consumed by that same frame's pass.

None of that is visible as latency on glass, and none of it changes the landmark *order* the S5 rig
asserted: start → answer → the map catches up. Where a scenario depends on the new shape, it says so.

## What is *not* automatable here, stated rather than discovered

**The banner's on-glass appearance is still not reachable by any gesture**, for the same reason S5
recorded: the freeze needs a live search *and* a map base, and Back on the planning screen posts the
cancellation in the same batch that puts the map base back. So [`Cycle`] **counts** banner repaints
and fails on more than one, but does not require one; the pixels are proven off-device by
`obc-sim --freeze --png`.

**Scenario G proves the fact, not a refusal.** S5 open question 2 was "`CoreMode`'s transfer level
does not see a route/trip/weather upload", and this slice closes it by feeding
`ExternalFacts::note_transfer` from the flat engine's own live transfer — which the board reports on
RTT as `xfer: transfer level active/idle`. Turning that level into a *named refusal* needs a consumer
of `Capabilities::navigator`, and today the only consumer of `Capabilities` at all is the weather
stage. So G asserts the level moves for a **route** upload (it never did before) and says out loud
that the refusal half belongs to Gate 4 / #1400. Claiming a refusal here would be claiming a rule the
firmware does not yet have.

Everything above `Link` is pure log analysis with no pyserial in it, which is what
`tools/tests/test_s6b_board_cutover_soak.py` drives against recorded RTT text.
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

# ── new with the typed executor (#1494) ──────────────────────────────────────────────────────────

# `CatalogEffect::RemoveObject`'s answer off the ticketed writer path. `existed false` is a
# **success** — the subject vanished before the commit and the goal state holds (#1433 §13).
REMOVED = "catalog: object"
REMOVE_FAILED = "removal failed"
# One completed `CatalogEffect::ReadCatalog`. The route line is the one every read emits first, so
# counting it counts re-reads — which is scenario F's whole question.
CATALOG_READ = "flat: Route menu loaded"
# The transfer level's edge, fed from the flat engine (S5 open question 2). `debug-uart` only.
XFER_ACTIVE = "xfer: transfer level active"
XFER_IDLE = "xfer: transfer level idle"
# The executor's own alarms. Any of these in a soak window is a failure: they are the shapes that
# used to be impossible because the drain performed everything itself.
EXEC_ALARMS = (
    "exec: an effect this board cannot serve",
    "answered twice in one frame",
    "came back on the legacy protocol",
    "a trip member read has no board half",
    "a sidecar write reached the board",
    "the executor paces the search",
)
# **Not an alarm**, deliberately: nothing consults `Capabilities::navigator` yet, so the ride menu's
# Detour row is still live and a rider pressing it produces an operation the board refuses by
# capability. That is an expected condition, and an alarm set that fails a run for one is an alarm
# set nobody trusts. It stays worth counting — a *rising* number across a soak would say the row
# should have been gated — so scenario K reports it.
DETOUR_REFUSED = "nav: detour is not supported on this board"

# The refusal string `nav_take_arena` answers a live cable transfer with. Scenario B's whole point:
# the rider must be told about the cable, not about "the scratch arena".
REFUSAL_TRANSFER = "a cable transfer holds the store"
REFUSAL_ARENA = "the scratch arena is busy"

# `deep_ride_margin_min` from `firmware/tools/resource_baseline.json` — scenario C fails if a
# reported stack peak eats into the margin. `STACK_RESERVE` is only the fallback for a line whose
# `/ M B` half could not be read; the board's own reported total is what the gate uses.
STACK_RESERVE = 65_536
DEEP_RIDE_MARGIN_MIN = 8_704


def alarms(lines: list[str]) -> list[str]:
    """Every executor alarm in the window. These are `defmt::error!`s the typed executor raises for
    a shape that cannot happen — an effect no board half serves, two answers for one domain in one
    frame, a class DeviceCore owns coming back on the legacy protocol. On a release image the
    matching `debug_assert!` is compiled out, so **this line is the only witness**."""
    return [line.strip() for line in lines if any(a in line for a in EXEC_ALARMS)]


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
    alarms: list[str] = field(default_factory=list)

    def verdict(self) -> str | None:
        """`None` when the cycle passed, else why it did not."""
        if self.alarms:
            return f"the executor raised an alarm: {self.alarms[0]}"
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
    cycle.alarms = alarms(lines)
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

    Under the typed executor the answer is a `NavigatorOutcome::PlanFinished` the *next* pass
    consumes, so the freeze is released one pass after `nav route:` rather than in the same one —
    and that next pass is immediate (`RideExec::owed`). The rule this asserts is unchanged, because
    it was always about ordering and never about which pass: a banner at or past the answer says the
    level outlived the run that owned it, and a map frame *before* the answer says the map plane drew
    while the nav arm was still out."""
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


def removals(lines: list[str]) -> list[bool]:
    """Every answered `CatalogEffect::RemoveObject` in the window, as its `existed` flag."""
    out = []
    for line in lines:
        if REMOVED in line and "removed (existed" in line:
            out.append("existed true" in line)
    return out


def catalog_reads(lines: list[str]) -> int:
    """How many completed catalog re-reads the window contains — scenario F counts these."""
    return sum(CATALOG_READ in line for line in lines)


def transfer_edges(lines: list[str]) -> list[str]:
    """The transfer level's edges in order: `"active"` / `"idle"`."""
    return ["active" if XFER_ACTIVE in line else "idle" for line in lines if XFER_ACTIVE in line or XFER_IDLE in line]


def stack_peaks(lines: list[str]) -> list[tuple[int, int]]:
    """Every `stack high-water N / M B` peak in the window, as `(used, total)`.

    **The `M` is not decoration.** It is `stackmeter::total()` — the *residual* main stack this
    flashed image actually has, which is what is left after `.bss`, `.data` and `.uninit` and is
    therefore several tens of kilobytes below the linker's `stack_reserve`. A gate that subtracted
    the peak from the reserve instead would report a comfortable margin on an image that has almost
    none, which is precisely the shape the FS8 record caught (37,016 used of 37,568 available)."""
    peaks: list[tuple[int, int]] = []
    for line in lines:
        if STACK_PEAK not in line:
            continue
        tail = line.split(STACK_PEAK, 1)[1]
        used, _, rest = tail.partition("/")
        used_digits = "".join(c for c in used if c.isdigit())
        total_digits = "".join(c for c in rest.split("B")[0] if c.isdigit())
        if used_digits:
            peaks.append((int(used_digits), int(total_digits) if total_digits else STACK_RESERVE))
    return peaks


def margin_verdict(peaks: list[tuple[int, int]]) -> str | None:
    """Scenario C's stack gate: the **smallest margin** any reported peak leaves must clear
    `deep_ride_margin_min`, measured against the stack the board says it has rather than the linker's
    reserve.

    The smallest margin, not the deepest peak: a DFU install reboots mid-soak, so one log can carry
    two images with different totals, and the deeper peak is not always the tighter one."""
    if not peaks:
        return None
    worst, total = min(peaks, key=lambda p: p[1] - p[0])
    margin = total - worst
    if margin < DEEP_RIDE_MARGIN_MIN:
        return f"stack peak {worst} B of {total} B leaves {margin} B, under the {DEEP_RIDE_MARGIN_MIN} B floor"
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
        # **Lesson 3.** The J-Link CDC does not carry anything for roughly the first second after
        # open; without this the very first probe below loses every tap and reports a wedge on a
        # perfectly healthy board.
        time.sleep(1.0)

    def send(self, line: str) -> None:
        self.port.write((line + "\n").encode())
        self.port.flush()

    def send_all(self, *lines: str) -> None:
        """Several debug-link messages in **one** `write()`.

        **Lesson 1**, and it is the difference between a tap and a hold: the recognizer times the
        gap between the down and the up edge, and a host-side `sleep` between two `write()`s is
        long enough under load to cross the 500 ms hold threshold. One write, one packet, one tap."""
        self.port.write(("".join(f"{line}\n" for line in lines)).encode())
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
        """Poll the log until `needle` appears after `mark`, or give up.

        **Lesson 4.** Every step waits on its own landmark rather than on a sleep long enough to
        cover the worst case: the plan phases run for hundreds of ms to seconds, and a fixed window
        wide enough for the slow path makes a 50-cycle run take an hour."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if any(needle in line for line in self.since(mark)):
                return True
            time.sleep(0.1)
        return False

    # -- the gestures and commands, in the debug-link vocabulary --

    def press(self) -> None:
        self.send_all("K s d", "K s u")

    def back(self) -> None:
        self.send_all("K b d", "K b u")

    def hold_select(self, seconds: float = 1.2) -> None:
        """A deliberate **hold** on Select — the hold-to-delete footer's gesture. The gap is the
        point here, so the two edges are two writes on purpose."""
        self.send("K s d")
        time.sleep(seconds)
        self.send("K s u")

    def step(self, n: int) -> None:
        self.send(f"K t {n}")

    def fix(self, lat_ud: int, lon_ud: int) -> None:
        self.send(f"F {lat_ud} {lon_ud}")

    def zoom(self, mpp: float) -> None:
        self.send(f"Z {mpp}")

    def plan(self, frm: tuple[int, int], to: tuple[int, int]) -> None:
        """`N <from_lon> <from_lat> <to_lon> <to_lat>` — **LON FIRST**, unlike the lat-first `F`."""
        self.send(f"N {frm[0]} {frm[1]} {to[0]} {to[1]}")


def liveness_probe(link: Link, retry: bool = True) -> None:
    """**Run this before trusting a single assertion.** The J-Link CDC wedges with `write()` still
    succeeding and RTT still flowing, and a blind script then passes every step against a board that
    heard nothing.

    Six taps must come back as the board's own `input: Step` acknowledgements. A grown log is *not*
    enough — sensors and frames keep logging through a wedge.

    **Lesson 3**: the first probe after a fresh open legitimately loses taps, so a zero is retried
    **once** before it is called a wedge. A second zero is real, and only a physical DK power-cycle
    clears it; fewer than six is a lossy cable, worth saying out loud but not worth sending someone
    to the power switch."""
    mark = link.mark()
    for _ in range(6):
        link.step(1)
        time.sleep(0.1)
    time.sleep(2.0)
    acks = step_acks(link.since(mark))
    if acks == 0 and retry:
        print("  liveness: no acknowledgements on the first probe — settling and retrying once")
        time.sleep(1.0)
        return liveness_probe(link, retry=False)
    if acks == 0:
        raise SystemExit(
            "VCOM is wedged: six injected taps produced no `input: Step` acknowledgement, twice.\n"
            "Power-cycle the DK physically (a re-flash does not clear it) and start again."
        )
    if acks < 6:
        print(f"  liveness: only {acks}/6 taps acknowledged — the cable is lossy, results may be noisy")


def reached_map(link: Link) -> bool:
    """Whether the base screen is the Map, probed the only way a quiet screen can be.

    A correct Map redraws nothing when nothing changed, so waiting for `map frame:` with no stimulus
    times out on a perfectly healthy one. `Z` pins the camera scale and forces exactly one redraw,
    and that redraw is a `map frame:` line **only** if the base actually draws the map."""
    mark = link.mark()
    link.zoom(22.0)
    return link.wait_for(mark, MAP_FRAME, timeout=2.0)


class Aborted(SystemExit):
    """The run cannot continue: the rig lost the screen it drives from."""


def heal_to_map(link: Link, attempts: int = 6) -> None:
    """**Lesson 2 — the per-cycle self-heal, with a loud abort.**

    Every cycle must start with the Map as the base screen. A cycle that ends somewhere else (a
    failure card the Back missed, a card that landed mid-cycle, a stray hold that navigated) poisons
    every cycle after it, and the run then reports 49 failures for one lost screen.

    The probe is a **zoom nudge**, not a bare wait: a map that is already correct redraws nothing on
    a quiet screen, so waiting for `map frame:` with no stimulus times out on a perfectly healthy
    Map. `Z` pins the camera scale and forces exactly one redraw, and that redraw is a `map frame:`
    line **only** if the base screen actually draws the map — which is the question. If it will not
    come back after `attempts` Backs, stop the run and say so instead of producing evidence about the
    wrong screen."""
    for _ in range(attempts):
        if reached_map(link):
            return
        link.back()
        time.sleep(0.3)
    raise Aborted(
        "ABORT — the rig could not get back to a map base after 6 Backs.\n"
        "Everything after this point would be evidence about the wrong screen. Re-flash, walk the\n"
        "device to the Map by hand, and re-run."
    )


# ── the scenarios ────────────────────────────────────────────────────────────────────────────────


def ride_to_map(link: Link, frm: tuple[int, int], to: tuple[int, int]) -> None:
    """Get a **map base** under the rig, whatever the card holds.

    The stored-route walk (Home → Route menu → overview → Map, three presses) is the fast path and
    the one a soak card normally takes. It assumes a stored route exists, and a freshly formatted
    card has none — the presses then land on an empty menu, the rig never reaches a map base, and
    every later scenario reports failures about the wrong screen. That is a **rig** gap, not a board
    one, and it cost a run.

    So: walk, probe, and if there is no map base, *make* a route rather than giving up — plan one
    with `N`, which needs no catalog at all. A finished plan is published, adopted and previewed, so
    one press off the overview opens the Map on it. Only if that fails too is the run stopped, with
    a message that names the precondition instead of leaving the operator to infer it from 50 failed
    cycles."""
    for _ in range(3):
        link.press()
        time.sleep(0.4)
    if reached_map(link):
        return

    print("  boot walk: no map base after the stored-route walk — planning one instead")
    print("             (an empty card has no route to open; the plan does not need one)")
    for _ in range(4):  # back out of whatever the presses landed on
        link.back()
        time.sleep(0.25)
    mark = link.mark()
    link.plan(frm, to)
    if link.wait_for(mark, PLAN_ANSWER, timeout=30.0) and "ok" in "".join(link.since(mark)):
        link.press()  # off the computed-route overview, onto the Map
        time.sleep(0.5)
        if reached_map(link):
            print("  boot walk: planned a route and opened it — map base reached")
            return

    raise Aborted(
        "ABORT — the rig could not reach a map base.\n"
        "Precondition: the mounted card needs a routable map covering the `--nav-from`/`--nav-to`\n"
        "coordinates, and either at least one stored route or a plan that succeeds between them.\n"
        "The defaults are the Freiburg N-S axis; the Grimsel coordinates the S5 rig shipped are\n"
        "outside this map's bbox and every plan there answers `no-path`.\n"
        "Check the RTT log for `nav route:` (did the plan run at all?) and `flat: Route menu loaded\n"
        "N route(s)` (does the card hold any?), then re-run."
    )


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

    The `N` posts a `NavigatorIntent::PlanRoute`; the pass that consumes it hands the executor an
    `Acquire`, which the **next** frame's store phase arms — so the wait for `nav plan: start` covers
    two frames now instead of one. It is bounded by the same 4 s either way (`RideExec::owed` asks
    for that second frame immediately).

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
        f"arena_refusals={len(cycle.arena_refusals)} plan_refusals={len(cycle.refusals)} "
        f"alarms={len(cycle.alarms)}"
    )
    return 1


def scenario_a(link: Link, cycles: int, frm: tuple[int, int], to: tuple[int, int], base: tuple[int, int]) -> int:
    """**A — the `render ⊥ nav` claim/release cycle on the new spine**, ≥50 times (S5 A, re-proven).

    Each cycle must arm the planner, answer, give the arm back and let the map catch up, with zero
    arena refusals and zero executor alarms in between. The failure it hunts is a map that stops
    redrawing — silent on a shipping build, which is why it is only visible here."""
    print(f"A: {cycles} plan cycles (render ⊥ nav claim/release, on `App::run_pass`)")
    ride_to_map(link, frm, to)
    stream_fixes(link, base, 3)
    failures = 0
    banners = 0
    for i in range(cycles):
        if i % 10 == 0:
            liveness_probe(link)
        heal_to_map(link)
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
    """**B — `nav ⊥ usb` and `render ⊥ usb`** (S5 B, re-proven). Semi-automatic: a hand on the cable.

    The plan offered during an upload must be refused *before* the planner arms, and the refusal must
    name the transfer — not "the scratch arena is busy", and not a `NoPath`. Under the typed executor
    the refusal is a `NavigatorOutcome::Failed { Workspace }` rather than a fabricated routing
    verdict, and the rider-visible card is the same generic failure tier.

    **The precondition, and it is the whole basis of the claim:** the refusal is reachable only while
    the USB **write-combining arm** is actually held, which the ride loop takes on
    `usb::stage_requested()` and announces as `arena: 64 KiB USB write-combining arm granted`. Without
    that grant the arena is free and the plan proceeds beside the upload — so the step below checks
    for the grant first and says so rather than failing the board."""
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
    """**C — the stuck-arm soak** (S5 C, re-proven). A continuous ride with a plan cycle every ~60 s
    and a periodic zoom nudge. Every released nav arm must be followed by a map render, the stack
    must never eat into `deep_ride_margin_min`, and no executor alarm may appear in an hour."""
    print(f"C: {minutes} minutes of continuous riding with a plan cycle every ~60 s")
    ride_to_map(link, frm, to)
    deadline = time.time() + minutes * 60
    failures = 0
    n = 0
    while time.time() < deadline:
        mark = link.mark()
        link.zoom(30.0 if n % 2 else 20.0)
        stream_fixes(link, (base[0] + n * 10, base[1]), 25)
        heal_to_map(link)
        cycle, lines = plan_cycle(link, frm, to, (base[0] + n * 10, base[1]))
        failures += report(f"cycle {n}", cycle, lines)
        window = link.since(mark)
        for problem in [margin_verdict(stack_peaks(window))] + faults(window) + alarms(window):
            if problem:
                failures += 1
                print(f"  cycle {n}: FAIL — {problem}")
        n += 1
    print(f"C: {n} cycles over {minutes} min, {failures} failures")
    return failures


def scenario_d(link: Link) -> int:
    """**D — typed catalog removal.** New to this slice: the rider's delete is a
    `CatalogEffect::RemoveObject` on the flat store's *answering* writer path, not a fire-and-forget
    post to a queue that drops on overflow.

    Semi-automatic, because only a human can say the row is gone from the menu. The rig's half is the
    protocol: exactly one answered removal per hold, `existed true` for a real object, and a re-issued
    delete of an object that is already gone answering `existed false` — a **success**, which is the
    one shape that must not read as a failure (#1433 §13)."""
    failures = 0
    print("D: walk to the Route menu, highlight a route you can lose, and hold Select to delete it.")
    mark = link.mark()
    input("   Press Enter the moment the hold fires. ")
    if not link.wait_for(mark, REMOVED, timeout=8.0):
        print("  FAIL — no `catalog: object … removed` line: the removal never reached the store")
        return failures + 1
    window = link.since(mark)
    seen = removals(window)
    if len(seen) != 1:
        print(f"  FAIL — {len(seen)} removals answered for one hold (expected exactly one)")
        failures += 1
    elif not seen[0]:
        print("  FAIL — the object was reported absent; the delete named something that was not there")
        failures += 1
    else:
        print("  one removal, answered, `existed true`: ok")
    if not link.wait_for(mark, CATALOG_READ, timeout=8.0):
        print("  FAIL — no catalog re-read followed the commit: the menu cannot have updated")
        failures += 1
    else:
        print("  the commit ordered one catalog re-read: ok")
    print("D: confirm on glass that the row is gone from the Route menu, then repeat for a ride.")
    input("   Press Enter when both are confirmed. ")
    return failures


def scenario_e(link: Link) -> int:
    """**E — removal under backpressure.** New to this slice, and the reason the removal left
    `MENU_DELETES`: that queue *drops* an id when it is full, which the domain would read as an
    operation that never completes — its one catalog slot occupied for the rest of the boot.

    On the answering path a busy writer costs a pass, never the delete. Semi-automatic: a hand on the
    phone or the cable."""
    print("E: start a large upload (cable or phone) so the one writer task is busy, then hold Select")
    print("   to delete a route while it streams.")
    mark = link.mark()
    input("   Press Enter the moment the hold fires. ")
    # Generous, because this is the whole point: the removal may wait several passes behind the
    # writer's queue and it must still land.
    if not link.wait_for(mark, REMOVED, timeout=30.0):
        print("  FAIL — the delete never completed while the writer was busy: the id was dropped")
        return 1
    window = link.since(mark)
    seen = removals(window)
    failures = 0
    if len(seen) != 1:
        print(f"  FAIL — {len(seen)} removals for one hold: a re-queued delete committed twice")
        failures += 1
    else:
        print("  the delete was re-queued and completed exactly once: ok")
    if any(REMOVE_FAILED in line for line in window):
        print("  note: the store refused at least once and the domain re-queued it — that is the design")
    return failures


def scenario_f(link: Link) -> int:
    """**F — one store commit, one catalog refresh.** New to this slice: the board reports
    `FlatStore::sequence()` as a *level* instead of counting commit edges into N `StoreChanged`
    events, so one upload orders exactly one re-read rather than one per commit.

    Semi-automatic: a hand on the phone."""
    failures = 0
    print("F: send one route from the companion. Press Enter when the received card appears.")
    mark = link.mark()
    input()
    time.sleep(3.0)
    reads = catalog_reads(link.since(mark))
    if reads == 0:
        print("  FAIL — the upload ordered no catalog re-read at all")
        failures += 1
    elif reads > 1:
        print(f"  FAIL — {reads} catalog re-reads for one upload: the level is being counted, not read")
        failures += 1
    else:
        print("  one upload, one catalog re-read: ok")

    print("F: now send the SAME route again (a same-id replace) while it is the active route.")
    mark = link.mark()
    input("   Press Enter when the card appears. ")
    time.sleep(3.0)
    reads = catalog_reads(link.since(mark))
    if reads != 1:
        print(f"  FAIL — {reads} catalog re-reads for one replace")
        failures += 1
    else:
        print("  the replace ordered one re-read too: ok")
    print("F: confirm on glass that the map redrew the replaced route (the displaced revision's")
    print("   geometry-derived state was dropped — the overview shape and the profile are the new ones).")
    input("   Press Enter when confirmed. ")
    return failures


def scenario_g(link: Link) -> int:
    """**G — `note_transfer` closes the S5 gap.** New to this slice, and read the module docs before
    reading the verdict: this proves the **fact**, not a refusal.

    Before S6b the transfer level came from `App::set_map_transfer`'s card, so only a *map* upload
    moved it. It now comes from the flat engine's own live transfer, so a route or trip upload moves
    it too — which is exactly what S5 open question 2 asked for. Turning the level into a named
    refusal needs a consumer of `Capabilities::navigator`, and that is Gate 4 / #1400."""
    print("G: send a ROUTE (not a map) from the companion. Press Enter when the transfer starts.")
    mark = link.mark()
    input()
    time.sleep(5.0)
    edges = transfer_edges(link.since(mark))
    if "active" not in edges:
        print("  FAIL — a route upload did not move the transfer level: the S5 gap is still open")
        return 1
    if "idle" not in edges[edges.index("active") :]:
        print("  FAIL — the level went active and never came back: a stuck transfer level")
        return 1
    print(f"  the route upload moved the transfer level and released it ({' → '.join(edges)}): ok")
    print("  (the *refusal* half needs a `Capabilities` consumer — Gate 4 / #1400, not this slice)")
    return 0


def scenario_h(link: Link, frm: tuple[int, int], to: tuple[int, int], base: tuple[int, int]) -> int:
    """**H — the recorder still closes** (the residual `FinishTrack`).

    The one open question this scenario settles: the pass applies the rider's Save and the **next**
    frame's residual drain performs it, so the reconcile still runs ahead of the tick that would
    otherwise append to a closing object. The footer totals must match the last samples, and the ride
    must appear in the menu."""
    print("H: start a ride, let it record for a minute, then Finish → Save.")
    ride_to_map(link, frm, to)
    stream_fixes(link, base, 60, delay=1.0)
    mark = link.mark()
    input("   Drive Finish → Save on the device, then press Enter. ")
    time.sleep(3.0)
    window = link.since(mark)
    failures = len(alarms(window))
    for a in alarms(window):
        print(f"  FAIL — {a}")
    if not link.wait_for(mark, CATALOG_READ, timeout=10.0):
        print("  FAIL — no catalog re-read after the save: the ride cannot have appeared in the menu")
        failures += 1
    else:
        print("  the save re-fed the catalogs: ok")
    print("H: confirm on glass that the ride is in the Rides menu and its distance/time match the")
    print("   last values the ride screen showed. Then record a second ride and Discard it.")
    input("   Press Enter when both are confirmed. ")
    return failures


def scenario_i(link: Link) -> int:
    """**I — settings, DFU and the card scan**: the three effects the executor serves under the store
    guard, each of which used to be a drained command."""
    failures = 0
    print("I: change a setting (units), power-cycle the device, and confirm it survived.")
    input("   Press Enter when confirmed. ")
    print("I: open System → free space and confirm the row fills with a figure (not `--`).")
    input("   Press Enter when confirmed. ")
    mark = link.mark()
    print("I: with UPDATE.BIN on the card, run System → Update. Confirm the check reaches the confirm")
    print("   card, then arm it and confirm the device reboots into the bootloader.")
    input("   Press Enter when the device has come back up. ")
    window = link.since(mark)
    for a in alarms(window):
        print(f"  FAIL — {a}")
        failures += 1
    if not failures:
        print("  no executor alarms across the scan/arm/reboot: ok")
    return failures


def scenario_j(link: Link, base: tuple[int, int]) -> int:
    """**J — wake and pace unchanged.** The typed executor asks for an immediate second pass whenever
    it is holding something (`RideExec::owed`); a parked screen must still be quiet.

    A parked Home screen should wake about once a minute (the clock minute-tick), not per tick. The
    rig counts frames, which is the observable proxy: more than a handful in two minutes means the
    loop is spinning."""
    print("J: leave the device on Home, untouched, for two minutes.")
    mark = link.mark()
    time.sleep(120)
    window = link.since(mark)
    frames = sum(MAP_FRAME in line or "ui frame:" in line for line in window)
    failures = 0
    if frames > 10:
        print(f"  FAIL — {frames} frames in 2 min on a parked screen: the loop is not sleeping")
        failures += 1
    else:
        print(f"  {frames} frames in 2 min on a parked Home screen: ok")
    print("J: now run a full-speed cable map upload and record its throughput from the builder.")
    input("   Press Enter when it finishes. ")
    print("   (S5 measured 7.5 MB/s on this path; record the figure on the issue.)")
    return failures


def scenario_k() -> int:
    """**K — the human-eye pass.** Nothing here is automatable, and the rig says so rather than
    pretending: banner legibility on the reflective panel, whether the whole-frame catch-up after a
    freeze looks like a catch-up rather than a glitch, and whether the hold bulge's confirm pop still
    lands as fast as it did (#348 — bulge-first is unchanged by this slice, and this is what says so
    to a person)."""
    print("K: human-eye checks — record the answers on #1494:")
    print("   0. If you pressed Detour: the board refuses it by capability (a `nav: detour is not")
    print("      supported` line, not an alarm). Confirm the spinner cleared and the map redrew.")
    print("   1. Is the Recalculating banner legible on the reflective panel, in daylight?")
    print("   2. Does the catch-up repaint after a freeze read as a catch-up, not as a glitch?")
    print("   3. After a hold fires, does the confirm pop still land immediately (bulge-first, #348)?")
    print("   4. Does anything feel a frame later than it used to — a delete, a menu, a plan?")
    input("   Press Enter when all four are recorded. ")
    return 0


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


SCENARIOS = "ABCDEFGHIJK"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("scenario", help=f"one or more of {SCENARIOS}, e.g. `A` or `DEFHIJ`")
    parser.add_argument("--port", help="VCOM device (default: the first /dev/cu.usbmodem*133)")
    parser.add_argument("--rtt-log", required=True, type=Path, help="the file `cargo rtt` is tee'd into")
    parser.add_argument("--cycles", type=int, default=50, help="scenario A plan cycles (>=50 is the gate)")
    parser.add_argument("--minutes", type=int, default=60, help="scenario C duration (60 is the gate)")
    # **Lesson 5 — in-map coordinates.** The Freiburg N–S axis, on the map this board mounts. The
    # Grimsel defaults the S5 rig shipped are outside the mounted bbox: every cycle answers
    # `no-path`, which reports a healthy board as a failure.
    parser.add_argument("--nav-from", type=coord, default=(7_849_000, 47_994_000), help="LON,LAT µdeg")
    parser.add_argument("--nav-to", type=coord, default=(7_846_000, 48_004_000), help="LON,LAT µdeg")
    parser.add_argument("--lat", type=int, default=47_994_000, help="streamed fix latitude, µdeg")
    parser.add_argument("--lon", type=int, default=7_849_000, help="streamed fix longitude, µdeg")
    args = parser.parse_args()

    unknown = [c for c in args.scenario.upper() if c not in SCENARIOS]
    if unknown:
        raise SystemExit(f"unknown scenario(s): {''.join(unknown)} (known: {SCENARIOS})")
    if not args.rtt_log.exists():
        raise SystemExit(f"{args.rtt_log} does not exist — start `DEFMT_LOG=debug cargo rtt … | tee` first")

    link = Link(resolve_port(args.port), args.rtt_log)
    liveness_probe(link)
    base = (args.lat, args.lon)
    failures = 0
    for name in args.scenario.upper():
        if name == "A":
            failures += scenario_a(link, args.cycles, args.nav_from, args.nav_to, base)
        elif name == "B":
            failures += scenario_b(link, args.nav_from, args.nav_to)
        elif name == "C":
            failures += scenario_c(link, args.minutes, args.nav_from, args.nav_to, base)
        elif name == "D":
            failures += scenario_d(link)
        elif name == "E":
            failures += scenario_e(link)
        elif name == "F":
            failures += scenario_f(link)
        elif name == "G":
            failures += scenario_g(link)
        elif name == "H":
            failures += scenario_h(link, args.nav_from, args.nav_to, base)
        elif name == "I":
            failures += scenario_i(link)
        elif name == "J":
            failures += scenario_j(link, base)
        else:
            failures += scenario_k()

    print("PASS" if failures == 0 else f"FAIL ({failures})")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
