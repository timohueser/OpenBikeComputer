#!/usr/bin/env python3
"""Author 5 m/s GPS motion and a 60 s stop from the accepted route's read-only CSV."""
import csv
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
import xml.etree.ElementTree as ET

namespace = "http://www.topografix.com/GPX/1/1"
ET.register_namespace("", namespace)
gpx = ET.Element(f"{{{namespace}}}gpx", version="1.1", creator="OpenBikeComputer accepted-route replay")
track = ET.SubElement(gpx, "trk")
ET.SubElement(track, "name").text = "Meiringen campsite accepted Visit replay"
segment = ET.SubElement(track, "trkseg")
start = datetime(2025, 6, 16, 10, tzinfo=timezone.utc)
dwell = 0
with Path(sys.argv[1]).open() as source:
    for row in csv.DictReader(source):
        repeats = 61 if row["stop"] == "1" else 1
        for pause in range(repeats):
            point = ET.SubElement(segment, "trkpt", lat=f'{int(row["lat"]) / 1e6:.6f}', lon=f'{int(row["lon"]) / 1e6:.6f}')
            if row["ele"]:
                ET.SubElement(point, "ele").text = row["ele"]
            time = start + timedelta(seconds=int(row["m"]) / 5 + dwell + pause)
            ET.SubElement(point, "time").text = time.isoformat(timespec="milliseconds").replace("+00:00", "Z")
        dwell += repeats - 1
ET.indent(gpx)
with Path(sys.argv[2]).open("xb") as output:
    ET.ElementTree(gpx).write(output, encoding="utf-8", xml_declaration=True)
