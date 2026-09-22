"""The staging recipes a frame asks for by `env`.

Ten functions against two hundred data rows is the right division, so these stay code: they copy
files, patch bytes at a fixed offset, rewrite the elevations in a GPX, prime a card with a simulator
run, and in one case join a terrain region into a map. Each one makes a directory and returns the
simulator arguments that point at it; the one that stages a map returns the map instead.

A recipe runs once per process and is then shared by every frame that names it, which is what the
frames expect: the two ride fixtures, the trip folder and the imported routes are one store each.
"""

from __future__ import annotations

import re
import shutil
import struct
from dataclasses import dataclass
from pathlib import Path

#: The `ride-1` footer distance, patched in place so the two same-day rides read differently on
#: the Rides rows. Distance is not part of the object's length validation, so the patched copy
#: still reads as a valid ride.
RIDE_DISTANCE = (72, 17800)


@dataclass(frozen=True)
class Staging:
    """What an environment gives a frame: extra simulator arguments, or a staged map."""

    args: tuple[str, ...] = ()
    map: str | None = None


class Stage:
    """The roots, a private directory for each recipe, and a quiet simulator."""

    def __init__(self, repo: Path, fixtures: Path, root: Path, run, script):
        self.repo = Path(repo)
        self.fixtures = Path(fixtures)
        self.vectors = self.repo / "specs" / "vectors"
        self._root = Path(root)
        self.run = run
        #: Expand a gesture script through the table's own `[script]` fragments.
        self.script = script

    def dir(self, name: str) -> Path:
        made = self._root / name
        made.mkdir(parents=True, exist_ok=True)
        return made


def routes(stage: Stage) -> Staging:
    """The two UI routes. The vector directory also holds deliberately invalid inputs."""
    where = stage.dir("routes")
    for name in ("route-plain.obcr", "route-waypoints.obcr"):
        shutil.copy(stage.vectors / name, where / name)
    return Staging(("--routes-dir", str(where)))


def plain_route(stage: Stage) -> Staging:
    """Only the waypoint-less `route-plain` vector route — a route whose corridor is empty."""
    where = stage.dir("plain-route")
    shutil.copy(stage.vectors / "route-plain.obcr", where / "route-plain.obcr")
    return Staging(("--routes-dir", str(where)))


def tracks(stage: Stage) -> Staging:
    """Two stored ride objects for the Rides screen, from the pinned `ride-v4.bin` protocol vector.

    Both rows are conservatively unsynced; flat synced and retention metadata belong to the later
    ride-domain boundary.
    """
    where = stage.dir("tracks")
    ride = (stage.vectors / "ride-v4.bin").read_bytes()
    (where / "ride-0.obcr").write_bytes(ride)
    patched = bytearray(ride)
    offset, distance = RIDE_DISTANCE
    struct.pack_into("<I", patched, offset, distance)
    (where / "ride-1.obcr").write_bytes(patched)
    return Staging(("--tracks-dir", str(where)))


def trips(stage: Stage) -> Staging:
    """A routes store with a trip folder: the two vector routes and grimsel-climb, named so their
    sorted-scan ids are 0/1/2, plus `TP1.OBT` ("Alpen Traverse", day routes [0, 1, 99], starting
    Monday 2025-09-29). The top level then shows one folder grouping ids 0+1 above the loose grimsel
    route.
    """
    where = stage.dir("trips")
    grimsel = stage.fixtures / "sim-grimsel" / "routes"
    shutil.copy(stage.vectors / "route-plain.obcr", where / "1-plain.obcr")
    shutil.copy(stage.vectors / "route-waypoints.obcr", where / "2-waypoints.obcr")
    shutil.copy(grimsel / "grimsel-climb.obcr", where / "3-grimsel.obcr")
    shutil.copy(grimsel / "TP1.OBT", where / "TP1.OBT")
    return Staging(("--routes-dir", str(where)))


def eta_route(stage: Stage) -> Staging:
    """The real Grimsel climb route, alone."""
    where = stage.dir("eta-route")
    shutil.copy(stage.fixtures / "sim-grimsel" / "routes" / "grimsel-climb.obcr", where / "grimsel-climb.obcr")
    return Staging(("--routes-dir", str(where)))


def eta_flat(stage: Stage) -> Staging:
    """A zero-elevation twin of the Grimsel climb — the same 19 km of geometry with every `<ele>`
    zeroed, imported through the simulator's own GPX path. One replay then drives both, so the only
    difference between the two ETA frames is the elevation. The twin also stands in for a
    device-planned route, whose points are all zero-elevation until terrain fills them.
    """
    where = stage.dir("eta-flat")
    source = stage.fixtures / "sim-grimsel" / "tracks" / "grimsel-climb.gpx"
    flat = where / "grimsel-flat.gpx"
    flat.write_text(re.sub(r"<ele>[^<]*</ele>", "<ele>0</ele>", source.read_text()))
    stage.run(["--import", str(flat), "--routes-dir", str(where)])
    flat.unlink()
    return Staging(("--routes-dir", str(where)))


def day_route(stage: Stage) -> Staging:
    """The real Grimsel climb, imported under a trip-day name so the start-away frames carry it."""
    where = stage.dir("day-route")
    source = stage.fixtures / "sim-grimsel" / "tracks" / "grimsel-climb.gpx"
    day = where / "Day 2 Ulrichen.gpx"
    shutil.copy(source, day)
    stage.run(["--import", str(day), "--routes-dir", str(where)])
    day.unlink()
    return Staging(("--routes-dir", str(where)))


def monaco_route(stage: Stage) -> Staging:
    """The real Monaco loop, imported at run time so no second `.obcr` is committed to re-cut on a
    format bump: a ~2.7 km line across central Monaco whose 300 m corridor catches real Resupply,
    Pharmacy and Lodging places, and whose waypoints cover five categories.
    """
    where = stage.dir("monaco-route")
    track = stage.fixtures / "sim-monaco" / "tracks" / "monaco-upahead.gpx"
    stage.run(["--import", str(track), "--routes-dir", str(where)])
    return Staging(("--routes-dir", str(where)))


def journey(stage: Stage) -> Staging:
    """A card carrying a real Cork destination and recording, so the ride recovery after a restart
    can reach the Journey resume card. The landmark script accepts a Visit, which persists both.
    """
    where = stage.dir("journey")
    card = where / "card.obc"
    cork = str(stage.fixtures / "sim-assistant-west-cork" / "west-cork.obcm")
    stage.run([cork, "--routes-dir", str(stage.dir("journey-routes")), "--create-card", str(card)])
    accept = stage.script("{landmarks} p p f p f p f")
    stage.run(
        # fmt: off
        ["--card", str(card), "--boot", "--heading", "0", "--center", "-9825560,51485575",
         "--script", accept, "--expect-screen", "Map", "--png", str(where / "accepted.png")],
        # fmt: on
    )
    return Staging(("--card", str(card)))


def elevation(stage: Stage) -> Staging:
    """A map with the terrain region joined in.

    The pinned Grimsel pack has a separate OBCT input. Staging it inside the OBCM makes the planner
    read terrain through the retained map object, as it does for an assembled device map. Registered
    fixture bytes stay unchanged: the joined copy is this directory's own.
    """
    source = stage.fixtures / "sim-grimsel" / "grimsel.obcm"
    output = stage.dir("elevation") / "terrain.obcm"
    body = bytearray(source.read_bytes())
    obcm = (stage.repo / "firmware" / "obc-formats" / "src" / "obcm.rs").read_text()
    version = int(re.search(r"pub const VERSION: u8 = (\d+);", obcm)[1])
    # The current map header: 16-byte units, with the terrain offset and length at bytes 41 and 45.
    assert len(body) >= 65 and body[:5] == b"OBCM" + bytes([version]) and body[40] == 4
    offset, length = struct.unpack_from("<II", body, 41)
    assert bool(offset) == bool(length), "incomplete terrain region"
    if not offset:
        terrain = source.with_suffix(".obcd").read_bytes()
        assert len(terrain) >= 64 and terrain[:5] == b"OBCT\x01", "expected the pinned OBCT v1 input"
        body.extend(b"\xff" * (-len(body) % 16))
        offset = len(body) // 16
        body.extend(terrain)
        body.extend(b"\xff" * (-len(body) % 16))
        length = len(body) // 16 - offset
        struct.pack_into("<II", body, 41, offset, length)
    assert (offset + length) * 16 <= len(body)
    output.write_bytes(body)
    return Staging(map=str(output))


ENVIRONMENTS = {
    "routes": routes,
    "plain-route": plain_route,
    "tracks": tracks,
    "trips": trips,
    "eta-route": eta_route,
    "eta-flat": eta_flat,
    "day-route": day_route,
    "monaco-route": monaco_route,
    "journey": journey,
    "elevation": elevation,
}
