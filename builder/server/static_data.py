"""Write the static web tier's data files.

The hosted builder (#894 phase C) has no backend, so the two documents the
FastAPI server computes on demand have to exist as files next to the app:

    regions.json   the trimmed + simplified Geofabrik download index — exactly
                   what GET /api/regions returns, so the picker is unchanged
    catalog.json   NOT written here: the map catalog manifest is the bakery's
                   output (`obc-pack catalog`, B1 #898), published wherever the
                   artifacts are

Usage (from the repo root, with the packer's virtualenv active):

    python -m builder.server.static_data --out builder/app/public/data

The frontend fetches both from `./data/` relative to the page by default;
VITE_DATA_BASE and VITE_CATALOG_URL move them (see platform/web.ts). The site
deploy (.github/workflows/deploy-site.yml) runs this into the built bundle.
"""
import argparse
import json
import os

from . import geofabrik

# A picker with no polygons is indistinguishable from a broken site, and it fails in
# the browser rather than here — so refuse to write one. The real index carries ~555
# downloadable regions (2026-07); anything under this floor means the upstream index
# changed shape, or a fetch half-succeeded, and the right answer is a failed deploy.
MIN_FEATURES = 200


class TooFewRegions(RuntimeError):
    """The Geofabrik index came back implausibly small (see MIN_FEATURES)."""


def write_regions(out_dir: str, min_features: int = MIN_FEATURES) -> tuple[str, int]:
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, "regions.json")
    index = geofabrik.get_regions()
    count = len(index.get("features", []))
    # Check before the rename, so a bad index never replaces a good one that is
    # already in place.
    if count < min_features:
        raise TooFewRegions(
            f"the Geofabrik index yielded {count} regions, below the floor of "
            f"{min_features} — refusing to write {path}"
        )
    # Temp file then rename: a half-written index served to a browser is a
    # picker with no regions and no explanation.
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(index, f)
    os.replace(tmp, path)
    return path, count


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", required=True, help="directory to write the data files into")
    ap.add_argument(
        "--min-features",
        type=int,
        default=MIN_FEATURES,
        help=f"refuse to write an index with fewer regions than this (default {MIN_FEATURES})",
    )
    args = ap.parse_args()
    path, count = write_regions(args.out, args.min_features)
    print(f"wrote {path} ({count} regions, {os.path.getsize(path) / (1 << 20):.1f} MB)")


if __name__ == "__main__":
    main()
