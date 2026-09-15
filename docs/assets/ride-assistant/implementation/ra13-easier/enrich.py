#!/usr/bin/env python3
"""Add actual native DEM samples to the pinned Swiss loop; preserve its OSM vertices."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET

p = argparse.ArgumentParser()
p.add_argument("sampler", type=Path)
p.add_argument("map", type=Path)
p.add_argument("source", type=Path)
p.add_argument("out", type=Path)
a = p.parse_args()
ns = "http://www.topografix.com/GPX/1/1"
ET.register_namespace("", ns)
tree = ET.parse(a.source)
segment = tree.find(".//{*}trkseg")
points = segment.findall("{*}trkpt")
assert len(points) == 307
assert points[76].find("{*}time").text == "2026-09-14T10:04:25Z"
coordinates = "".join(f"{round(float(n.attrib['lat']) * 1e6)} {round(float(n.attrib['lon']) * 1e6)}\n" for n in points)
result = subprocess.run([str(a.sampler.resolve()), str(a.map)], input=coordinates, text=True, capture_output=True, check=True)
heights = [int(z) for z in result.stdout.splitlines()]
assert len(heights) == len(points)
for point, height in zip(points, heights):
    assert point.find("{*}ele") is None
    elevation = ET.Element(f"{{{ns}}}ele")
    elevation.text = str(height)
    point.insert(0, elevation)
a.out.mkdir(parents=True, exist_ok=True)
route = a.out / "meiringen-loop-dem.gpx"
tree.write(route, encoding="utf-8", xml_declaration=True)
for point in points[:76]:
    segment.remove(point)
motion = a.out / "meiringen-loop-from-265.gpx"
tree.write(motion, encoding="utf-8", xml_declaration=True)
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
record = {"map_sha256": digest(a.map), "source_gpx_sha256": digest(a.source), "samples": len(heights),
          "min_elevation_m": min(heights), "max_elevation_m": max(heights), "motion_first_source_point": 76,
          "route_gpx_sha256": digest(route), "motion_gpx_sha256": digest(motion)}
(a.out / "provenance.json").write_text(json.dumps(record, indent=2) + "\n")
