"""The staging recipes a frame asks for by `env`.

Ten functions against two hundred data rows is the right division, so these stay code: they copy
files, patch bytes at a fixed offset, rewrite the elevations in a GPX, prime a card with a simulator
run, and in one case join a terrain region into a map. Each one makes a directory and returns the
simulator arguments that point at it; the one that stages a map returns the map instead.

A recipe runs once per process and is then shared by every frame that names it, which is what the
frames expect: the two ride fixtures, the trip folder and the imported routes are one store each.
"""

from __future__ import annotations

import calendar
import math
import re
import shutil
import struct
from dataclasses import dataclass
from pathlib import Path

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


def _ride(samples, name, start, distance_m, moving_s, climb_m, avg_hr, trip):
    """A ride object as `specs/obc-ble-interface-spec.md` §7.2 lays it out: 20-byte samples, then
    the 150-byte footer. `samples` are `(lon, lat, ele)`; `trip` is `(key, day index, day count,
    name)` or `None`.
    """
    body = bytearray()
    for i, (lon, lat, ele) in enumerate(samples):
        body += struct.pack("<iihHIBBH", lon, lat, ele, i == 0, i * 5000, 0xFF, 0xFF, 0xFFFF)
    raw = name.encode()
    footer = struct.pack("<4sBBHIIIHHHI", b"OBRF", 5, len(raw), 150, start, distance_m, moving_s, 0, climb_m, 0,
                         len(samples))
    footer += struct.pack("<BBBBHHI48s", avg_hr or 0xFF, 0xFF, 0xFF, 0, 0xFFFF, 0xFFFF, 0xFFFF_FFFF, raw)
    key, day, days, trip_name = trip or (0, 0, 0, "")
    footer += struct.pack("<QBBBB48s", key, day, days, 1, len(trip_name.encode()), trip_name.encode())
    return bytes(body + footer)


def _metric(a, b):
    """Two `(lon, lat)` microdegree points as local metres, for a ground distance."""
    k = 0.111_32 * math.cos(math.radians(a[1] / 1e6))
    return (a[0] * k, a[1] * 0.111_32), (b[0] * k, b[1] * 0.111_32)


def tracks(stage: Stage) -> Staging:
    """Four stored rides for the Rides screens, oldest first, so the import gives the newest the
    highest id: two loose copies of the pinned `ride-v5.bin` vector, "Sensor Ride" with all three
    sensors, and two days of the trip "Alps traverse" on the Grimsel climb's track. Day 2 has a
    heart rate; Day 1 has no sensor. Every ride is unsynced; the flat store stages no archive rows.
    """
    where = stage.dir("tracks")
    vector = (stage.vectors / "ride-v5.bin").read_bytes()
    footer = len(vector) - 150
    for index, distance in enumerate((12_345, 17_800)):
        loose = bytearray(vector)
        struct.pack_into("<I", loose, footer + 12, distance)
        loose[footer + 90 : footer + 150] = bytes(60)
        loose[footer + 100] = vector[footer + 100]
        (where / f"ride-{index}.obcr").write_bytes(loose)
    gpx = (stage.fixtures / "sim-grimsel" / "tracks" / "grimsel-climb.gpx").read_text()
    points = re.findall(r'<trkpt lat="([-\d.]+)" lon="([-\d.]+)">\s*<ele>([-\d.]+)</ele>', gpx)
    climb = [(round(float(lon) * 1e6), round(float(lat) * 1e6), round(float(ele))) for lat, lon, ele in points[::3]]
    assert len(climb) > 100, "the Grimsel track parses"
    alps = (0x0123_4567_89AB_CDEF, "Alps traverse")
    days = [
        # name, samples, start (UTC), moving time, average heart rate: Day 1 rides the climb down
        ("Day 1 Andermatt", climb[::-1], (2025, 9, 29, 7, 30), 48 * 60, None),
        ("Day 2 Ulrichen", climb, (2025, 9, 30, 8, 12), 95 * 60, 128),
    ]
    for day, (name, samples, when, moving, hr) in enumerate(days):
        metres = round(sum(math.dist(*_metric(a, b)) for a, b in zip(samples, samples[1:])))
        ascent = sum(max(0, b[2] - a[2]) for a, b in zip(samples, samples[1:]))
        start = calendar.timegm((*when, 0, 0, 0, 0))
        ride = _ride(samples, name, start, metres, moving, ascent, hr, (alps[0], day, 3, alps[1]))
        (where / f"ride-{day + 2}.obcr").write_bytes(ride)
    return Staging(("--tracks-dir", str(where)))


def _shift_route_lon(route: bytearray, dlon: int) -> None:
    """Move a waypoint-less OBCR east by `dlon` µdeg (`specs/OBCR_Spec.md`: the header start and
    bbox, and each chunk meta's bbox and anchor). Points are deltas from the anchor, so they follow.
    """
    for off in (8, 16, 24):
        struct.pack_into("<i", route, off, struct.unpack_from("<i", route, off)[0] + dlon)
    chunks, index = struct.unpack_from("<II", route, 52)
    for k in range(chunks):
        for off in (0, 8, 16):
            at = index + 48 * k + off
            struct.pack_into("<i", route, at, struct.unpack_from("<i", route, at)[0] + dlon)


def _rename_route(route: bytearray, name: str) -> None:
    """Rewrite an OBCR header's name (`specs/OBCR_Spec.md` §1: Name Len at 6, Name at 64)."""
    raw = name.encode()
    route[6] = len(raw)
    route[64:112] = raw.ljust(48, b"\0")


def trips(stage: Stage) -> Staging:
    """A routes store with a trip folder: the two vector routes and grimsel-climb, named so their
    sorted-scan ids are 0/1/2, plus `TP1.OBT` ("Alpen Traverse", day routes [0, 1, 99], starting
    Monday 2025-09-29). The top level then shows one folder grouping ids 0+1 above the loose grimsel
    route. The two vector routes carry day names, so the day list's rows differ.
    """
    where = stage.dir("trips")
    grimsel = stage.fixtures / "sim-grimsel" / "routes"
    # Both vector routes run 29,000 µdeg east from the same start. Day 1 moves west by that much,
    # so it ends where Day 2 starts and no transfer lies between them.
    for source, target, name, shift in [
        ("route-plain.obcr", "1-plain.obcr", "Day 1 Andermatt", -29_000),
        ("route-waypoints.obcr", "2-waypoints.obcr", "Day 2 Ulrichen", 0),
    ]:
        route = bytearray((stage.vectors / source).read_bytes())
        _rename_route(route, name)
        _shift_route_lon(route, shift)
        (where / target).write_bytes(route)
    shutil.copy(grimsel / "grimsel-climb.obcr", where / "3-grimsel.obcr")
    shutil.copy(grimsel / "TP1.OBT", where / "TP1.OBT")
    return Staging(("--routes-dir", str(where)))


def trip_week(stage: Stage) -> Staging:
    """A seven-day trip, "Alpen Traverse Nord", on seven renamed copies of the plain vector route,
    starting Monday 2025-09-29. Its day list scrolls, so the title bar shows the counter. The trip
    object is written here as `specs/obc-ble-interface-spec.md` §7.7 lays it out; the simulator
    maps each day's route index to the imported route.
    """
    where = stage.dir("trip-week")
    towns = ["Andermatt", "Ulrichen", "Brig", "Visp", "Sierre", "Sion", "Martigny"]
    for day, town in enumerate(towns, 1):
        route = bytearray((stage.vectors / "route-plain.obcr").read_bytes())
        _rename_route(route, f"Day {day} {town}")
        (where / f"day-{day}.obcr").write_bytes(route)
    name = "Alpen Traverse Nord".encode()
    trip = struct.pack("<BBHB48sBHQ", 3, 0, len(towns), len(name), name, 0, 20_360, 2)
    for index in range(len(towns)):
        trip += struct.pack("<QII", index, 0, 0xFFFF_FFFF)
    (where / "TP2.OBT").write_bytes(trip)
    return Staging(("--routes-dir", str(where)))


def arrival_trip(stage: Stage) -> Staging:
    """A three-day trip whose Day 2 is the real Grimsel climb, so a replay of that climb rides to the
    end of a trip day: "Day 1 Andermatt" (the plain vector route), "Day 2 Ulrichen" (the climb) and
    "Day 3 Brig" (the climb reversed, so it starts where Day 2 ends), in the trip "Alpen Traverse"
    as `TP1.OBT`.
    """
    where = stage.dir("arrival-trip")
    climb = (stage.fixtures / "sim-grimsel" / "routes" / "grimsel-climb.obcr").read_bytes()
    for day, (source, name) in enumerate(
        [((stage.vectors / "route-plain.obcr").read_bytes(), "Day 1 Andermatt"), (climb, "Day 2 Ulrichen")], 1
    ):
        route = bytearray(source)
        _rename_route(route, name)
        (where / f"day-{day}.obcr").write_bytes(route)
    track = (stage.fixtures / "sim-grimsel" / "tracks" / "grimsel-climb.gpx").read_text()
    points = re.findall(r"<trkpt\b.*?(?:/>|</trkpt>)", track, flags=re.S)
    points = [re.sub(r"<time>[^<]*</time>", "", point) for point in reversed(points)]
    descent = where / "Day 3 Brig.gpx"
    descent.write_text("<gpx><trk><trkseg>" + "".join(points) + "</trkseg></trk></gpx>")
    stage.run(["--import", str(descent), "--routes-dir", str(where)])
    descent.unlink()
    (where / "Day 3 Brig.obcr").rename(where / "day-3.obcr")
    name = "Alpen Traverse".encode()
    trip = struct.pack("<BBHB48sBHQ", 3, 0, 3, len(name), name, 0, 20_360, 1)
    for index in range(3):
        trip += struct.pack("<QII", index, 0, 0xFFFF_FFFF)
    (where / "TP1.OBT").write_bytes(trip)
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


def long_route(stage: Stage) -> Staging:
    """A synthetic day of about 80 km across the Grimsel map and beyond, imported through the
    simulator's GPX path. It fits the route overview's map band past the band's map scale ceiling,
    so page 1 draws the track on the plain page.
    """
    where = stage.dir("long-route")
    track = where / "long-day.gpx"
    points = []
    for i in range(61):
        t = i / 60
        lat = 46.40 + 0.32 * t + 0.04 * math.sin(t * 6 * math.pi)
        lon = 8.05 + 0.57 * t
        ele = 1500 + 800 * math.sin(t * 3 * math.pi)
        points.append(f'<trkpt lat="{lat:.5f}" lon="{lon:.5f}"><ele>{ele:.0f}</ele></trkpt>')
    track.write_text("<gpx><trk><trkseg>" + "".join(points) + "</trkseg></trk></gpx>")
    stage.run(["--import", str(track), "--routes-dir", str(where)])
    track.unlink()
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


def monaco_garden(stage: Stage) -> Staging:
    """A short Monaco line past the pharmacy with split hours and a two-line name, so the Up-ahead
    detail can show its fullest page. The track is replayed as well as imported.
    """
    where = stage.dir("monaco-garden")
    track = where / "garden.gpx"
    points = "".join(
        f'<trkpt lat="43.73470" lon="{7.4110 + 0.002 * i:.4f}"><time>2025-01-06T09:{i:02d}:00Z</time></trkpt>'
        for i in range(6)
    )
    track.write_text("<gpx><trk><trkseg>" + points + "</trkseg></trk></gpx>")
    stage.run(["--import", str(track), "--routes-dir", str(where)])
    return Staging(("--routes-dir", str(where), "--gpx", str(track)))


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
    "trip-week": trip_week,
    "arrival-trip": arrival_trip,
    "eta-route": eta_route,
    "eta-flat": eta_flat,
    "day-route": day_route,
    "long-route": long_route,
    "monaco-route": monaco_route,
    "monaco-garden": monaco_garden,
    "journey": journey,
    "elevation": elevation,
}
