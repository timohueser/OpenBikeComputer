#!/usr/bin/env python3
"""Build the companion website fixture from the simulator's canonical Grimsel GPX files.

The imported plan is the full route the device stores; the finished ride is the shorter replay the
browser actually drives, so the draft can finish before the summit without inventing geometry.
Change ROUTE_GPX, RIDE_GPX, and DISPLAY_NAME when the showcase route changes, then run this script
(the screenshot capture script also runs it). `--check` is CI's drift guard.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_DIR = SCRIPT_DIR.parent.parent
ROUTE_GPX = REPO_DIR / "fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx"
RIDE_GPX = REPO_DIR / "fixtures/sources/sim-grimsel/tracks/grimsel-climb-demo.gpx"
DEMO_RS = REPO_DIR / "apps/obc-web-demo/src/demo.rs"
DISPLAY_NAME = "Grimsel Pass"
FIXTURE_DIR = REPO_DIR / "companion-ios/Packages/OBCKit/Sources/OBCMock/Fixtures"
FIXTURE_JSON = FIXTURE_DIR / "website.json"
IMPORT_GPX = FIXTURE_DIR / "website-import.gpx"
GPX_NS = "http://www.topografix.com/GPX/1/1"


def parse_time(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def iso_time(value: dt.datetime) -> str:
    return value.astimezone(dt.timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def distance_m(a: dict[str, object], b: dict[str, object]) -> float:
    radius = 6_371_000.0
    lat1 = math.radians(float(a["lat"]))
    lat2 = math.radians(float(b["lat"]))
    dlat = lat2 - lat1
    dlon = math.radians(float(b["lon"]) - float(a["lon"]))
    h = math.sin(dlat / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin(dlon / 2) ** 2
    return 2 * radius * math.asin(math.sqrt(h))


def sample(points: list[dict[str, object]], maximum: int = 96) -> list[dict[str, object]]:
    if len(points) <= maximum:
        return points
    indices = {round(i * (len(points) - 1) / (maximum - 1)) for i in range(maximum)}
    return [points[i] for i in sorted(indices)]


def load_gpx(path: Path, until_seconds: float | None = None) -> tuple[list[dict[str, object]], dt.datetime, dt.datetime]:
    root = ET.parse(path).getroot()
    points: list[dict[str, object]] = []
    times: list[dt.datetime] = []
    for node in root.findall(f".//{{{GPX_NS}}}trkpt"):
        elevation = node.findtext(f"{{{GPX_NS}}}ele")
        timestamp = node.findtext(f"{{{GPX_NS}}}time")
        if elevation is None or timestamp is None:
            continue
        parsed_time = parse_time(timestamp)
        if times and until_seconds is not None and (parsed_time - times[0]).total_seconds() > until_seconds:
            break
        points.append({
            "lat": round(float(node.attrib["lat"]), 6),
            "lon": round(float(node.attrib["lon"]), 6),
            "ele": round(float(elevation), 1),
        })
        times.append(parsed_time)
    if len(points) < 2:
        raise SystemExit(f"{path} contains fewer than two timestamped track points")
    return points, times[0], times[-1]


def ride_finish_seconds() -> float:
    match = re.search(r"const TOUR_BASELINE_S: f64 = ([0-9.]+);", DEMO_RS.read_text())
    if not match:
        raise SystemExit(f"could not read TOUR_BASELINE_S from {DEMO_RS}")
    return float(match.group(1))


def stats(points: list[dict[str, object]], started: dt.datetime, ended: dt.datetime) -> tuple[int, int, int]:
    distance = round(sum(distance_m(a, b) for a, b in zip(points, points[1:])))
    climb = round(sum(max(0.0, float(b["ele"]) - float(a["ele"])) for a, b in zip(points, points[1:])))
    return distance, climb, round((ended - started).total_seconds())


def fixture(
    route_points: list[dict[str, object]], route_started: dt.datetime, route_ended: dt.datetime,
    ride_points: list[dict[str, object]], ride_started: dt.datetime, ride_ended: dt.datetime,
) -> dict[str, object]:
    route_distance, route_climb, route_moving = stats(route_points, route_started, route_ended)
    ride_distance, ride_climb, ride_moving = stats(ride_points, ride_started, ride_ended)
    route_compact = sample(route_points)
    ride_compact = sample(ride_points)
    return {
        "deviceInfo": {
            "name": "Trailhead",
            "firmwareVersion": "0.4.2",
            "hardwareVersion": "nRF54LM20 rev B",
            "serial": "OBC-WEBSITE-01",
            "storeEpoch": 197132289,
            "protocolVersion": 4,
        },
        "config": {"name": "Trailhead", "units": "metric"},
        "battery": 82,
        "diagnostics": "Landing-page Grimsel fixture\n",
        "routes": [{
            "id": "grimsel-pass",
            "deviceObjectID": 7,
            "name": DISPLAY_NAME,
            "distanceMeters": route_distance,
            "elevationGainMeters": route_climb,
            "estimatedDuration": route_moving,
            "source": "gpx",
            "payloadBytes": max(1, route_distance * 12),
            "track": route_compact,
            "waypoints": [
                {
                    "name": "Start",
                    "note": "Guttannen",
                    "distanceAlongMeters": 0,
                    "lat": route_compact[0]["lat"],
                    "lon": route_compact[0]["lon"],
                },
                {
                    "name": "Grimsel Pass",
                    "note": "Finish",
                    "distanceAlongMeters": route_distance,
                    "lat": route_compact[-1]["lat"],
                    "lon": route_compact[-1]["lon"],
                },
            ],
        }],
        "rides": [{
            "id": "ride-grimsel-pass",
            "name": DISPLAY_NAME,
            "date": iso_time(ride_started),
            "distanceMeters": ride_distance,
            "movingTime": ride_moving,
            "averageSpeedMps": round(ride_distance / ride_moving, 4),
            "climbMeters": ride_climb,
            "payloadBytes": max(1, ride_distance * 20),
            "track": ride_compact,
        }],
    }


def import_gpx(source: Path) -> bytes:
    text = source.read_text()
    root = ET.fromstring(text)
    names = root.findall(f".//{{{GPX_NS}}}name")
    if names:
        old_name = names[0].text or ""
        text = text.replace(f"<name>{old_name}</name>", f"<name>{DISPLAY_NAME}</name>")
    return text.encode()


def reconcile(path: Path, content: bytes, check: bool) -> bool:
    current = path.read_bytes() if path.exists() else None
    if current == content:
        return True
    if check:
        print(f"stale generated website fixture: {path.relative_to(REPO_DIR)}", file=sys.stderr)
        return False
    path.write_bytes(content)
    print(f"updated {path.relative_to(REPO_DIR)}")
    return True


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    route_points, route_started, route_ended = load_gpx(ROUTE_GPX)
    # The phone receives exactly the prefix the embedded device pre-rolls before the visible
    # Pause → Finish sequence. Read the cutoff from Rust so changing that draft endpoint cannot
    # silently make the two surfaces disagree.
    ride_points, ride_started, ride_ended = load_gpx(RIDE_GPX, ride_finish_seconds())
    fixture_bytes = (json.dumps(fixture(
        route_points, route_started, route_ended, ride_points, ride_started, ride_ended
    ), indent=2, ensure_ascii=False) + "\n").encode()
    ok = reconcile(FIXTURE_JSON, fixture_bytes, args.check)
    ok = reconcile(IMPORT_GPX, import_gpx(ROUTE_GPX), args.check) and ok
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
