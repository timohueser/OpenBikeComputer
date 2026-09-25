#!/usr/bin/env python3
"""Generate web, desktop and iOS branding from assets/brand/signpost.svg.

Run with Python 3 and ImageMagick 7. macOS iconutil also produces the ICNS bundle.
"""
import json
import math
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
BRAND = ROOT / "assets/brand"
ET.register_namespace("", "http://www.w3.org/2000/svg")


def geometry():
    """Flatten board rotations for SVG rasterizers with different transform support."""
    paths = list(ET.parse(BRAND / "signpost.svg").getroot())
    for path in paths:
        transform = path.attrib.pop("transform", None)
        if not transform:
            continue
        angle, cx, cy = map(float, re.findall(r"-?[\d.]+", transform))
        angle = math.radians(angle)
        points = []
        x = y = 0
        for command, values in re.findall(r"([MLHVZ])([^MLHVZ]*)", path.attrib["d"]):
            numbers = list(map(float, re.findall(r"-?[\d.]+", values)))
            if command in ("M", "L"):
                pairs = list(zip(numbers[::2], numbers[1::2]))
            elif command == "H":
                pairs = [(n, y) for n in numbers]
            elif command == "V":
                pairs = [(x, n) for n in numbers]
            else:
                continue
            for x, y in pairs:
                points.append((cx + (x-cx)*math.cos(angle) - (y-cy)*math.sin(angle),
                               cy + (x-cx)*math.sin(angle) + (y-cy)*math.cos(angle)))
        path.set("d", "M" + "L".join(f"{x:.6f} {y:.6f}" for x, y in points) + "Z")
    return "\n".join(ET.tostring(p, encoding="unicode") for p in paths)


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text + "\n")


def svg(shapes, *, dark=False, tile=False, rounded=False, adaptive=False):
    shapes = shapes.replace("#5c5a2e", "#bdb47e" if dark else "#5c5a2e")
    edge = 128 if tile else 100
    background = ""
    if tile:
        background = f'<rect width="128" height="128" rx="{29 if rounded else 0}" fill="{"#16150f" if dark else "#f4f2eb"}"/>'
        shapes = f'<g transform="translate(6.5 8.8) scale(1.15)">{shapes}</g>'
    style = '<style>@media(prefers-color-scheme:dark){path:first-of-type{fill:#bdb47e}}</style>' if adaptive else ""
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 {edge} {edge}">{style}{background}{shapes}</svg>'


def png(source, destination, size):
    destination.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["magick", "-background", "none", str(source), "-resize", f"{size}x{size}",
                    "-depth", "8", "-strip", f"PNG32:{destination}"], check=True)


def main():
    shapes = geometry()
    with tempfile.TemporaryDirectory(prefix="obc-brand-") as scratch:
        scratch = Path(scratch)
        paper = scratch / "paper.svg"
        ink = scratch / "ink.svg"
        write(paper, svg(shapes, tile=True))
        write(ink, svg(shapes, tile=True, dark=True))
        for folder in [ROOT / "docs/assets/brand", ROOT / "builder/app/public/brand",
                       ROOT / "apps/obc-verification/static/brand"]:
            write(folder / "signpost.svg", svg(shapes, adaptive=True))
            write(folder / "app-icon.svg", svg(shapes, tile=True))
            png(paper, folder / "apple-touch-icon.png", 180)
        app = ROOT / "companion-ios/OBCCompanion/Assets.xcassets/AppIcon.appiconset"
        png(paper, app / "AppIcon.png", 1024)
        png(ink, app / "AppIcon-dark.png", 1024)
        # iOS masks the opaque square artwork itself.
        for name in ("AppIcon.png", "AppIcon-dark.png"):
            subprocess.run(["magick", str(app / name), "-alpha", "off", "-strip", f"PNG24:{app / name}"], check=True)
        images = ROOT / "companion-ios/Packages/OBCKit/Sources/OBCUI/Resources/Brand.xcassets/Signpost.imageset"
        for dark in (False, True):
            source = scratch / "mark.svg"
            write(source, svg(shapes, dark=dark))
            png(source, images / ("signpost-dark.png" if dark else "signpost.png"), 300)
        entries = [{"idiom": "universal", "filename": "signpost.png"},
                   {"idiom": "universal", "filename": "signpost-dark.png",
                    "appearances": [{"appearance": "luminosity", "value": "dark"}]}]
        write(images / "Contents.json", json.dumps({"images": entries, "info": {"author": "xcode", "version": 1}}, indent=2))
        desktop = ROOT / "apps/obc-desktop/icons"
        tile = scratch / "desktop.svg"
        write(tile, svg(shapes, tile=True, rounded=True))
        for filename, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)]:
            png(tile, desktop / filename, size)
        subprocess.run(["magick", str(desktop / "icon.png"), "-define", "icon:auto-resize=256,48,32,16", str(desktop / "icon.ico")], check=True)
        if shutil.which("iconutil"):
            iconset = scratch / "OBC.iconset"
            for size in (16, 32, 128, 256, 512):
                for scale in (1, 2):
                    png(tile, iconset / f'icon_{size}x{size}{"@2x" if scale == 2 else ""}.png', size * scale)
            subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(desktop / "icon.icns")], check=True)
    print("Generated website, desktop and iOS brand assets.")


if __name__ == "__main__":
    main()
