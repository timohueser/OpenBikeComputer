"""Make the night sky of the TrailView engine: `milkyway.webp` and `stars.bin` in
builder/web/src/lib/trailview/light/.

    uv run --locked --group trailview-sky python -m tools.trailview.sky

The sources are `nasa-svs-milky-way` and `yale-bright-stars` of data/sources.toml. The script
downloads each into ~/.cache/obc/trailview/sky once and fails unless its SHA-256 is the source's
`start`.

- `milkyway.webp`: the Milky Way without its bright stars, in linear light encoded as sRGB, plate
  carrée in J2000 right ascension and declination: right ascension 0 h in the middle and growing to
  the left, declination +90° at the top.
- `stars.bin`: per star of the catalogue with a J2000 position and V ≤ MAX_V, brightest first, 6
  bytes little-endian: right ascension u16 (a turn in 65 536 steps), declination i16 (±90° in ±32 767
  steps), V u8 ((V + 2) × 20), B−V u8 ((B−V + 0.5) × 80). `light/sky.ts` reads it.
"""

import gzip
import hashlib
import io
import struct
import sys
import tomllib
import urllib.request
from pathlib import Path

import numpy as np
import OpenEXR
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "builder/web/src/lib/trailview/light"
CACHE = Path.home() / ".cache/obc/trailview/sky"
# 0.18° a texel: the engine draws the Milky Way as a glow behind the stars of the catalogue.
WIDTH, HEIGHT = 2048, 1024
QUALITY = 70
# The faintest stars that an eye sees under the darkest sky.
MAX_V = 6.5
# B−V where the catalogue has none: a star like the sun.
DEFAULT_BV = 0.65


def source(id: str) -> Path:
    """The file of a source, from the cache or its URL, checked against its pinned SHA-256."""
    entry = next(s for s in tomllib.loads((ROOT / "data/sources.toml").read_text())["source"] if s["id"] == id)
    url = entry["fetch"]["url"]
    path = CACHE / url.rsplit("/", 1)[1]
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(url, timeout=300) as response:
            path.write_bytes(response.read())
    if hashlib.sha256(path.read_bytes()).hexdigest() != entry["start"]:
        sys.exit(f"{path}: its SHA-256 is not the `start` of `{id}` in data/sources.toml")
    return path


def milky_way(exr: Path) -> bytes:
    """The Milky Way, box-filtered to WIDTH × HEIGHT, as lossy WebP."""
    rgb = OpenEXR.File(str(exr)).parts[0].channels["RGB"].pixels.astype(np.float32)
    h, w, _ = rgb.shape
    rgb = np.maximum(rgb.reshape(HEIGHT, h // HEIGHT, WIDTH, w // WIDTH, 3).mean(axis=(1, 3)), 0)
    srgb = np.where(rgb <= 0.0031308, 12.92 * rgb, 1.055 * np.power(rgb, 1 / 2.4) - 0.055)
    out = io.BytesIO()
    Image.fromarray((np.clip(srgb, 0, 1) * 255 + 0.5).astype(np.uint8)).save(out, "WEBP", quality=QUALITY, method=6)
    return out.getvalue()


def stars(catalogue: Path) -> bytes:
    """The stars of the Yale Bright Star Catalogue, by the byte columns of the CDS V/50 ReadMe."""
    rows = []
    for line in gzip.decompress(catalogue.read_bytes()).decode("ascii").splitlines():
        line = line.ljust(197)
        if not line[75:77].strip() or not line[102:107].strip() or float(line[102:107]) > MAX_V:
            continue
        ra = (int(line[75:77]) + int(line[77:79]) / 60 + float(line[79:83]) / 3600) / 24
        dec = (1 if line[83] == "+" else -1) * (int(line[84:86]) + int(line[86:88]) / 60 + int(line[88:90]) / 3600)
        bv = float(line[109:114]) if line[109:114].strip() else DEFAULT_BV
        rows.append((float(line[102:107]), ra, dec, bv))
    rows.sort()
    code = lambda value, scale, offset: min(max(round((value + offset) * scale), 0), 255)
    return b"".join(
        struct.pack("<HhBB", round(ra * 65536) % 65536, round(dec / 90 * 32767), code(v, 20, 2), code(bv, 80, 0.5))
        for v, ra, dec, bv in rows
    )


def main() -> None:
    webp, table = milky_way(source("nasa-svs-milky-way")), stars(source("yale-bright-stars"))
    (OUT / "milkyway.webp").write_bytes(webp)
    (OUT / "stars.bin").write_bytes(table)
    print(f"milkyway.webp: {len(webp)} bytes; stars.bin: {len(table) // 6} stars, {len(table)} bytes")


if __name__ == "__main__":
    main()
