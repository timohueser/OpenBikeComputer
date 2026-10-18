"""The baseline regions of the Europe tune: RESOLVE's terrestrial ecoregions (Dinerstein et al.
2017, CC BY 4.0) that cover more than MIN_KM2 of European land, clipped to it and simplified to
about a kilometre, with the countries each one crosses, written to tune/ecoregions.json.

    uv run --with shapely==2.1.2 --with pyogrio==0.13.0 --with pyproj==3.8.0 python tune/ecoregions.py

Europe is Natural Earth's European countries west of 60° E and south of 72° N; the Canaries,
Madeira and the Azores come with Spain and Portugal.
"""
import json
import re
import urllib.request
import zipfile
from pathlib import Path

import pyogrio
import shapely
import shapely.ops
from pyproj import Transformer

HERE = Path(__file__).parent
SOURCES = Path.home() / ".cache/obc/trail-view/_sources/ecoregions"
ECOREGIONS = "https://storage.googleapis.com/teow2016/Ecoregions2017.zip"
COUNTRIES = "https://naciscdn.org/naturalearth/50m/cultural/ne_50m_admin_0_countries.zip"
MIN_KM2 = 500
SIMPLIFY_DEG = 0.01
CREDIT = "Ecoregions 2017 © Resolve, CC BY 4.0; borders: Natural Earth"


def unpacked(url: str) -> Path:
    """The shapefile inside a zip, downloaded and unpacked once."""
    archive = SOURCES / url.rsplit("/", 1)[1]
    folder = archive.with_suffix("")
    if not folder.exists():
        SOURCES.mkdir(parents=True, exist_ok=True)
        if not archive.exists():
            urllib.request.urlretrieve(url, archive)
        zipfile.ZipFile(archive).extractall(folder)
    return next(folder.rglob("*.shp"))


def read(path: Path, columns: list[str]) -> list[tuple[dict, shapely.Geometry]]:
    meta, _, wkb, fields = pyogrio.raw.read(path, columns=columns)
    return [({c: f[i] for c, f in zip(meta["fields"], fields)}, g) for i, g in enumerate(shapely.from_wkb(wkb))]


def main() -> None:
    clip = shapely.box(-32, 27, 60, 72)
    countries = [(row["NAME"], shapely.intersection(shapely.make_valid(g), clip))
                 for row, g in read(unpacked(COUNTRIES), ["NAME", "CONTINENT"]) if row["CONTINENT"] == "Europe"]
    countries = [(name, g) for name, g in countries if not g.is_empty]
    europe = shapely.union_all([g for _, g in countries])
    to_laea = Transformer.from_crs("EPSG:4326", "EPSG:3035", always_xy=True).transform
    km2 = lambda g: shapely.ops.transform(to_laea, g).area / 1e6

    regions = []
    for row, g in read(unpacked(ECOREGIONS), ["ECO_ID", "ECO_NAME", "BIOME_NAME"]):
        if not shapely.intersects(g, clip):
            continue
        part = shapely.intersection(shapely.make_valid(g), europe)
        if part.is_empty or km2(part) < MIN_KM2:
            continue
        shape = shapely.simplify(part, SIMPLIFY_DEG, preserve_topology=True)
        polygons = [p for p in getattr(shape, "geoms", [shape]) if isinstance(p, shapely.Polygon) and not p.is_empty]
        regions.append({
            "id": int(row["ECO_ID"]),
            "slug": re.sub(r"[^a-z0-9]+", "-", row["ECO_NAME"].lower()).strip("-"),
            "name": row["ECO_NAME"],
            "biome": row["BIOME_NAME"],
            "km2": round(km2(part)),
            "countries": sorted(name for name, c in countries if shapely.intersection(c, part).area > 1e-4),
            # GeoJSON MultiPolygon coordinates, [lon, lat] to 4 decimals.
            "polygons": [[[[round(x, 4), round(y, 4)] for x, y in ring.coords] for ring in [p.exterior, *p.interiors]]
                         for p in polygons],
        })
    regions.sort(key=lambda r: r["name"])
    (HERE / "ecoregions.json").write_text(json.dumps({"credit": CREDIT, "regions": regions}, ensure_ascii=False, separators=(",", ":")))
    for r in regions:
        print(f"{r['id']:4} {r['km2']:8} km²  {r['name']}  [{', '.join(r['countries'])}]")
    print(f"{len(regions)} ecoregions")


if __name__ == "__main__":
    main()
