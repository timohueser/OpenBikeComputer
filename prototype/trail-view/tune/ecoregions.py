"""The baseline regions of the Europe tune: the RESOLVE terrestrial ecoregions (Dinerstein et al.
2017, CC BY 4.0) in SHORT, clipped to European land and simplified to about a kilometre, with
their short names and the countries each one crosses, written to tune/ecoregions.json.

    uv run --with shapely==2.1.2 --with pyogrio==0.13.0 --with pyproj==3.8.0 python tune/ecoregions.py

Europe is Natural Earth's European countries west of 60° E and south of 72° N; the Canaries,
Madeira and the Azores come with Spain and Portugal. SHORT holds every ecoregion with more than
500 km² of it, except Rock and Ice and those that lie only in Russia.
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
CLIP = shapely.box(-32, 27, 60, 72)
SIMPLIFY_DEG = 0.01
CREDIT = "Ecoregions 2017 © Resolve, CC BY 4.0; borders: Natural Earth"
# The baseline by RESOLVE ECO_ID: ecoregion 689 is regions/alps.json.
SHORT = {
    785: "aegean", 689: "alps", 644: "apennines", 645: "azores", 646: "balkan", 647: "baltic", 691: "caledon",
    787: "canaries", 648: "cantabria", 692: "carpathians", 651: "celtic", 654: "central-europe", 788: "corsica",
    789: "crete", 660: "dinaric", 661: "forest-steppe", 663: "english-lowlands", 664: "atlantic", 729: "faroe",
    792: "iberian-conifer", 793: "iberia", 711: "iceland", 794: "illyria", 795: "italy-med", 774: "kola",
    668: "madeira", 796: "canaries-east", 672: "north-atlantic", 799: "catalan-med", 800: "nw-iberia",
    674: "pannonia", 801: "pindus", 675: "po", 735: "pontic", 676: "pyrenees", 678: "rhodope", 679: "sarmatic",
    780: "fjell", 717: "taiga", 708: "norway-coast", 802: "south-apennines", 803: "se-iberia", 805: "sw-iberia",
    806: "tyrrhenian", 686: "western-europe",
}


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


def europe() -> list[tuple[dict, shapely.Geometry]]:
    """Natural Earth's European countries cut to CLIP: each one's row and shape."""
    countries = [(row, shapely.intersection(shapely.make_valid(g), CLIP))
                 for row, g in read(unpacked(COUNTRIES), ["NAME", "CONTINENT", "ISO_A2_EH"]) if row["CONTINENT"] == "Europe"]
    return [(row, g) for row, g in countries if not g.is_empty]


def polygons(shape: shapely.Geometry) -> list:
    """The shape simplified, as GeoJSON MultiPolygon coordinates: [lon, lat] to 4 decimals."""
    shape = shapely.simplify(shape, SIMPLIFY_DEG, preserve_topology=True)
    return [[[[round(x, 4), round(y, 4)] for x, y in ring.coords] for ring in [p.exterior, *p.interiors]]
            for p in getattr(shape, "geoms", [shape]) if isinstance(p, shapely.Polygon) and not p.is_empty]


def main() -> None:
    countries = [(row["NAME"], g) for row, g in europe()]
    land = shapely.union_all([g for _, g in countries])
    to_laea = Transformer.from_crs("EPSG:4326", "EPSG:3035", always_xy=True).transform
    km2 = lambda g: shapely.ops.transform(to_laea, g).area / 1e6

    regions = []
    for row, g in read(unpacked(ECOREGIONS), ["ECO_ID", "ECO_NAME", "BIOME_NAME"]):
        if int(row["ECO_ID"]) not in SHORT:
            continue
        part = shapely.intersection(shapely.make_valid(g), land)
        regions.append({
            "id": int(row["ECO_ID"]),
            "short": SHORT[int(row["ECO_ID"])],
            "slug": re.sub(r"[^a-z0-9]+", "-", row["ECO_NAME"].lower()).strip("-"),
            "name": row["ECO_NAME"],
            "biome": row["BIOME_NAME"],
            "km2": round(km2(part)),
            "countries": sorted(name for name, c in countries if shapely.intersection(c, part).area > 1e-4),
            "polygons": polygons(part),
        })
    regions.sort(key=lambda r: r["name"])
    (HERE / "ecoregions.json").write_text(json.dumps({"credit": CREDIT, "regions": regions}, ensure_ascii=False, separators=(",", ":")))
    for r in regions:
        print(f"{r['id']:4} {r['short']:17} {r['km2']:8} km²  {r['name']}  [{', '.join(r['countries'])}]")
    print(f"{len(regions)} ecoregions")


if __name__ == "__main__":
    main()
