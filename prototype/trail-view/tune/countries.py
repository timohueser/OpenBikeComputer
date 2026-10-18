"""The places bake.py names in meta.country and meta.subdivision: Natural Earth's 1:50m admin-0
countries of Europe, as tune/ecoregions.py cuts it, and the USA, and the 1:10m first-level
subdivisions of those European countries, simplified to about a kilometre, written to
tune/countries.json.

    uv run --with shapely==2.1.2 --with pyogrio==0.13.0 --with pyproj==3.8.0 python tune/countries.py
"""
import json
from collections import defaultdict

import shapely

from ecoregions import CLIP, COUNTRIES, HERE, europe, polygons, read, unpacked

SUBDIVISIONS = "https://naciscdn.org/naturalearth/10m/cultural/ne_10m_admin_1_states_provinces.zip"
# Natural Earth lists these countries by province or département; their regions are the first level.
BY_REGION = {"ES", "FR", "IT"}
CREDIT = "Borders: Natural Earth"


def places(shapes: list[tuple[str, shapely.Geometry]]) -> list[dict]:
    """Each code's shapes as one flat list of rings: bake.py tests a point even-odd over every ring,
    so a hole needs no flag."""
    out = []
    for iso, g in shapes:
        out.append({"iso": iso, "rings": [ring for polygon in polygons(g) for ring in polygon]})
        print(f"{iso:8} {len(out[-1]['rings']):4} rings {sum(map(len, out[-1]['rings'])):6} points")
    return sorted(out, key=lambda p: p["iso"])


def main() -> None:
    usa = [(row, shapely.make_valid(g)) for row, g in read(unpacked(COUNTRIES), ["ISO_A2_EH"]) if row["ISO_A2_EH"] == "US"]
    european = europe()
    countries = places([(row["ISO_A2_EH"].lower(), g) for row, g in european + usa])
    if odd := [c["iso"] for c in countries if len(c["iso"]) != 2]:
        raise SystemExit(f"no ISO code: {odd}")

    isos = {row["ISO_A2_EH"] for row, _ in european}
    shapes = defaultdict(list)
    for row, g in read(unpacked(SUBDIVISIONS), ["iso_a2", "iso_3166_2", "region_cod"]):
        code = row["region_cod"] if row["iso_a2"] in BY_REGION else row["iso_3166_2"]
        # Natural Earth marks a country without subdivisions with a placeholder such as MC-X01~.
        if row["iso_a2"] in isos and code and "~" not in code:
            shapes[code.strip().replace(".", "-").lower()].append(shapely.make_valid(g))
    clipped = [(code, shapely.intersection(shapely.union_all(gs), CLIP)) for code, gs in shapes.items()]
    subdivisions = places([(code, g) for code, g in clipped if not g.is_empty])

    (HERE / "countries.json").write_text(json.dumps({"credit": CREDIT, "countries": countries, "subdivisions": subdivisions},
                                                    separators=(",", ":")))
    print(f"{len(countries)} countries, {len(subdivisions)} subdivisions")


if __name__ == "__main__":
    main()
