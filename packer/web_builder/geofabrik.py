"""Fetch, cache, and simplify the Geofabrik download index.

The index (index-v1.json) is a GeoJSON FeatureCollection describing every
downloadable region and subregion (countries, German Bundesländer, US states,
...) with a `pbf` download URL and a boundary geometry. The raw file carries
very detailed boundaries for hundreds of regions, so we simplify the geometries
and trim the properties before handing them to the browser, caching the result
to disk so the work happens only once.
"""
import json
import os
import re
import time

import requests
from shapely.geometry import shape, mapping

from . import paths

INDEX_URL = "https://download.geofabrik.de/index-v1.json"

# Derived from OBCM_CACHE_DIR (see paths.py).
CACHE_DIR = paths.GEOFABRIK_CACHE
RAW_PATH = os.path.join(CACHE_DIR, "index-v1.json")
SIMPLIFIED_PATH = os.path.join(CACHE_DIR, "regions-simplified.json")

# Simplification tolerance in degrees. ~0.01 deg ≈ 1 km, plenty for an
# overview world map while keeping the payload small.
SIMPLIFY_TOLERANCE = 0.01

# Refresh the raw index at most once a week.
MAX_AGE_SECONDS = 7 * 24 * 3600

_HTML_TAG = re.compile(r"<[^>]+>")


def _clean_name(name):
    # A few Geofabrik names embed HTML (e.g. "Nord-Norge<br />(Northern Norway)").
    return _HTML_TAG.sub(" ", name or "").replace("  ", " ").strip()


def _ensure_raw_index():
    os.makedirs(CACHE_DIR, exist_ok=True)
    fresh = (
        os.path.exists(RAW_PATH)
        and (time.time() - os.path.getmtime(RAW_PATH)) < MAX_AGE_SECONDS
    )
    if fresh:
        return
    resp = requests.get(INDEX_URL, timeout=120)
    resp.raise_for_status()
    with open(RAW_PATH, "w") as f:
        f.write(resp.text)


def _build_simplified():
    with open(RAW_PATH) as f:
        raw = json.load(f)

    features = []
    for feat in raw.get("features", []):
        props = feat.get("properties", {})
        pbf_url = (props.get("urls") or {}).get("pbf")
        if not pbf_url:
            continue  # only keep regions we can actually download
        try:
            geom = shape(feat["geometry"]).simplify(SIMPLIFY_TOLERANCE)
        except Exception:
            geom = shape(feat["geometry"])
        features.append(
            {
                "type": "Feature",
                "properties": {
                    "id": props.get("id"),
                    "name": _clean_name(props.get("name")),
                    "parent": props.get("parent"),
                    "pbf_url": pbf_url,
                },
                "geometry": mapping(geom),
            }
        )

    # Mark which regions have children so the UI can offer expansion.
    parents = {f["properties"]["parent"] for f in features if f["properties"]["parent"]}
    for f in features:
        f["properties"]["has_children"] = f["properties"]["id"] in parents

    fc = {"type": "FeatureCollection", "features": features}
    with open(SIMPLIFIED_PATH, "w") as f:
        json.dump(fc, f)
    return fc


def get_regions(force: bool = False):
    """Return the trimmed/simplified FeatureCollection, building it if needed."""
    _ensure_raw_index()
    raw_mtime = os.path.getmtime(RAW_PATH) if os.path.exists(RAW_PATH) else 0
    have_simplified = os.path.exists(SIMPLIFIED_PATH)
    stale = have_simplified and os.path.getmtime(SIMPLIFIED_PATH) < raw_mtime
    if force or not have_simplified or stale:
        return _build_simplified()
    with open(SIMPLIFIED_PATH) as f:
        return json.load(f)


def region_pbf_urls(region_ids):
    """Map a list of region ids to their .pbf download URLs (ordered, deduped)."""
    fc = get_regions()
    by_id = {f["properties"]["id"]: f["properties"]["pbf_url"] for f in fc["features"]}
    urls = []
    seen = set()
    for rid in region_ids:
        if rid not in by_id:
            raise KeyError(f"Unknown region id: {rid}")
        url = by_id[rid]
        if url not in seen:
            seen.add(url)
            urls.append((rid, url))
    return urls
