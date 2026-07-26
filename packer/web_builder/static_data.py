"""Write the static web tier's data files.

The hosted builder (#894 phase C) has no backend, so the two documents the
FastAPI server computes on demand have to exist as files next to the app:

    regions.json   the trimmed + simplified Geofabrik download index — exactly
                   what GET /api/regions returns, so the picker is unchanged
    catalog.json   NOT written here: the map catalog manifest is the bakery's
                   output (`obc-pack catalog`, B1 #898), published wherever the
                   artifacts are

Usage (from the repo root, with the packer's virtualenv active):

    python -m packer.web_builder.static_data --out packer/web_builder/frontend/public/data

The frontend fetches both from `./data/` relative to the page by default;
VITE_DATA_BASE and VITE_CATALOG_URL move them (see platform/web.ts). C6 (#905)
is what wires this into the deploy.
"""
import argparse
import json
import os

from . import geofabrik


def write_regions(out_dir: str) -> str:
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, "regions.json")
    # Temp file then rename: a half-written index served to a browser is a
    # picker with no regions and no explanation.
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(geofabrik.get_regions(), f)
    os.replace(tmp, path)
    return path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", required=True, help="directory to write the data files into")
    args = ap.parse_args()
    path = write_regions(args.out)
    print(f"wrote {path} ({os.path.getsize(path) / (1 << 20):.1f} MB)")


if __name__ == "__main__":
    main()
