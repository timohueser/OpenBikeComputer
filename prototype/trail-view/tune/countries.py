"""The countries bake.py names in meta.country: Natural Earth's 1:50m admin-0 countries of Europe,
as tune/ecoregions.py cuts it, and the USA, simplified to about a kilometre, written to
tune/countries.json.

    uv run --with shapely==2.1.2 --with pyogrio==0.13.0 --with pyproj==3.8.0 python tune/countries.py
"""
import json

import shapely

from ecoregions import COUNTRIES, HERE, europe, polygons, read, unpacked

CREDIT = "Borders: Natural Earth"


def main() -> None:
    usa = [(row, shapely.make_valid(g)) for row, g in read(unpacked(COUNTRIES), ["ISO_A2_EH"]) if row["ISO_A2_EH"] == "US"]
    countries = []
    for row, g in europe() + usa:
        # One flat list: bake.py tests a point even-odd over every ring, so a hole needs no flag.
        rings = [ring for polygon in polygons(g) for ring in polygon]
        countries.append({"iso": row["ISO_A2_EH"].lower(), "rings": rings})
        print(f"{countries[-1]['iso']}  {len(rings):4} rings {sum(map(len, rings)):6} points")
    if odd := [c["iso"] for c in countries if len(c["iso"]) != 2]:
        raise SystemExit(f"no ISO code: {odd}")
    countries.sort(key=lambda c: c["iso"])
    (HERE / "countries.json").write_text(json.dumps({"credit": CREDIT, "countries": countries}, separators=(",", ":")))
    print(f"{len(countries)} countries")


if __name__ == "__main__":
    main()
