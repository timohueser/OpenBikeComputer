#!/usr/bin/env python3
"""Fetch a finer reference DEM for `obc-dem surface --reference`.

Copernicus GLO-30 is the best free *global* elevation source, but 30 m cannot hold a rock tower:
at Engelberg its bilinear surface runs 100 m below the Hahnen's summit. National mapping agencies
publish LiDAR one to two orders finer, and most of it is now open data. This script pulls a box
from one of those services and writes plain WGS84 GeoTIFFs — the only thing the baker wants.

The baker treats a reference as optional per area, so coverage stopping at a border is normal and
needs no special handling: cells the reference misses come out byte-identical to a run without it.

    python3 fetch_reference.py --list
    python3 fetch_reference.py --source ch --bbox 8.38,46.78,8.55,46.90 --out ref/
    obc-dem surface native.obcd crest.obcd --reference ref/

Every source here answers without an API key or a login. Sources that need one are listed in
README.md instead, because a bakery step that stops to ask for a credential is not a bakery step.
"""

import argparse
import json
import math
import os
import sys
import time
import urllib.parse
import urllib.request

import numpy as np
import rasterio
from rasterio.io import MemoryFile
from rasterio.merge import merge
from rasterio.warp import Resampling, calculate_default_transform, reproject
from pyproj import Transformer

TIMEOUT = 300
# One request per tile; services cap the pixels they will return, and a failed 4000 x 4000 request
# wastes far more time than four 2000 x 2000 ones.
MAX_PIXELS = 2000


class Source:
    def __init__(self, key, country, name, kind, resolution_m, licence, attribution, extent, **kw):
        self.key, self.country, self.name, self.kind = key, country, name, kind
        self.resolution_m, self.licence, self.attribution, self.extent = resolution_m, licence, attribution, extent
        self.__dict__.update(kw)

    def covers(self, bbox):
        w, s, e, n = bbox
        a, b, c, d = self.extent
        return not (e < a or w > c or n < b or s > d)


SOURCES = [
    Source(
        "ch", "Switzerland", "swissALTI3D (swisstopo)", "stac", 2.0,
        "Open data, attribution required",
        "© swisstopo",
        (5.9, 45.8, 10.5, 47.9),
        stac="https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d/items",
        gsd="2",
    ),
    Source(
        "fr", "France", "RGE ALTI (IGN)", "wms_bil", 1.0,
        "Licence Ouverte / Open Licence",
        "© IGN",
        (-5.3, 41.3, 9.6, 51.1),
        url="https://data.geopf.fr/wms-r/wms",
        layer="ELEVATION.ELEVATIONGRIDCOVERAGE.HIGHRES",
    ),
    Source(
        "us", "United States", "3DEP (USGS)", "arcgis", 1.0,
        "Public domain",
        "USGS 3DEP",
        (-179.0, 17.0, -65.0, 72.0),
        url="https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer/exportImage",
    ),
    Source(
        "no", "Norway", "NHM DTM (Kartverket)", "wcs10", 1.0,
        "CC BY 4.0",
        "© Kartverket",
        (4.0, 57.8, 31.5, 71.5),
        url="https://wcs.geonorge.no/skwms1/wcs.hoyde-dtm-nhm-25833",
        coverage="nhm_dtm_topo_25833",
        epsg=25833,
    ),
    Source(
        "es", "Spain", "MDT05 (IGN/PNOA LiDAR)", "wcs20", 5.0,
        "CC BY 4.0",
        "© Instituto Geográfico Nacional",
        (-18.2, 27.6, 4.4, 43.9),
        url="https://servicios.idee.es/wcs-inspire/mdt",
        coverage="Elevacion4258_5",
        epsg=4326,
        axes=("long", "lat"),
    ),
    Source(
        "nl", "Netherlands", "AHN DTM (PDOK)", "wcs20", 0.5,
        "CC BY 4.0",
        "© Rijkswaterstaat / AHN",
        (3.2, 50.7, 7.3, 53.6),
        url="https://service.pdok.nl/rws/ahn/wcs/v1_0",
        coverage="dtm_05m",
        epsg=28992,
        axes=("x", "y"),
    ),
    Source(
        "de-nw", "Germany (NRW)", "DGM1 (Geobasis NRW)", "wcs20", 1.0,
        "dl-de/zero-2-0",
        "© Geobasis NRW",
        (5.8, 50.3, 9.5, 52.6),
        url="https://www.wcs.nrw.de/geobasis/wcs_nw_dgm",
        coverage="nw_dgm",
        epsg=25832,
        axes=("x", "y"),
    ),
]
BY_KEY = {s.key: s for s in SOURCES}


def get(url, timeout=TIMEOUT):
    """A service drops the occasional request and a bake pulls hundreds, so retry before failing."""
    for delay in (2, 4, 8, None):
        try:
            with urllib.request.urlopen(url, timeout=timeout) as r:
                return r.read()
        except Exception:
            if delay is None:
                raise
            time.sleep(delay)


def tiles(bbox, resolution_m, lat):
    """Split a WGS84 box into requests no larger than MAX_PIXELS on a side."""
    w, s, e, n = bbox
    span_x = (e - w) * 111320.0 * math.cos(math.radians(lat))
    span_y = (n - s) * 111320.0
    nx = max(1, math.ceil(span_x / resolution_m / MAX_PIXELS))
    ny = max(1, math.ceil(span_y / resolution_m / MAX_PIXELS))
    for i in range(ny):
        for j in range(nx):
            yield (w + (e - w) * j / nx, s + (n - s) * i / ny, w + (e - w) * (j + 1) / nx, s + (n - s) * (i + 1) / ny)


def clamp(count):
    return max(1, min(int(count), MAX_PIXELS))


def pixels(bbox, resolution_m, lat):
    w, s, e, n = bbox
    span_x = (e - w) * 111320.0 * math.cos(math.radians(lat))
    return clamp(span_x / resolution_m), clamp((n - s) * 111320.0 / resolution_m)


def fetch_arcgis(src, bbox, resolution_m, lat):
    px, py = pixels(bbox, resolution_m, lat)
    q = urllib.parse.urlencode({
        "bbox": ",".join(f"{v}" for v in bbox), "bboxSR": 4326, "size": f"{px},{py}", "imageSR": 4326,
        "format": "tiff", "pixelType": "F32", "interpolation": "RSP_BilinearInterpolation", "f": "image",
    })
    return get(f"{src.url}?{q}")


def fetch_wcs20(src, bbox, resolution_m, lat):
    """WCS 2.0 answers at the coverage's native step unless asked otherwise, and half-metre LiDAR
    overruns every server's size cap, so the output size is always stated."""
    w, s, e, n = bbox
    if src.epsg == 4326:
        (lo_x, lo_y), (hi_x, hi_y) = (w, s), (e, n)
        span_x, span_y = (e - w) * 111320.0 * math.cos(math.radians(lat)), (n - s) * 111320.0
    else:
        t = Transformer.from_crs("EPSG:4326", f"EPSG:{src.epsg}", always_xy=True)
        (lo_x, lo_y), (hi_x, hi_y) = t.transform(w, s), t.transform(e, n)
        span_x, span_y = hi_x - lo_x, hi_y - lo_y
    ax, ay = src.axes
    px, py = clamp(span_x / resolution_m), clamp(span_y / resolution_m)
    q = (f"{src.url}?service=WCS&version=2.0.1&request=GetCoverage&coverageId={src.coverage}"
         f"&subset={ax}({lo_x},{hi_x})&subset={ay}({lo_y},{hi_y})"
         f"&scalesize={ax}({px}),{ay}({py})&format=image/tiff")
    return get(q)


def fetch_wcs10(src, bbox, resolution_m, lat):
    w, s, e, n = bbox
    t = Transformer.from_crs("EPSG:4326", f"EPSG:{src.epsg}", always_xy=True)
    (lo_x, lo_y), (hi_x, hi_y) = t.transform(w, s), t.transform(e, n)
    px, py = clamp((hi_x - lo_x) / resolution_m), clamp((hi_y - lo_y) / resolution_m)
    q = (f"{src.url}?service=WCS&version=1.0.0&request=GetCoverage&coverage={src.coverage}"
         f"&crs=EPSG:{src.epsg}&bbox={lo_x},{lo_y},{hi_x},{hi_y}&width={px}&height={py}&format=GeoTIFF")
    return get(q)


def fetch_wms_bil(src, bbox, resolution_m, lat):
    """IGN serves RGE ALTI as raw float32 over WMS, so the GeoTIFF is assembled here."""
    px, py = pixels(bbox, resolution_m, lat)
    w, s, e, n = bbox
    q = urllib.parse.urlencode({
        "SERVICE": "WMS", "VERSION": "1.3.0", "REQUEST": "GetMap", "LAYERS": src.layer, "STYLES": "",
        "CRS": "EPSG:4326", "BBOX": f"{s},{w},{n},{e}", "WIDTH": px, "HEIGHT": py,
        "FORMAT": "image/x-bil;bits=32",
    })
    raw = get(f"{src.url}?{q}")
    a = np.frombuffer(raw, dtype="<f4").reshape(py, px).copy()
    a[a < -1000] = np.nan
    profile = {
        "driver": "GTiff", "height": py, "width": px, "count": 1, "dtype": "float32", "crs": "EPSG:4326",
        "transform": rasterio.transform.from_bounds(w, s, e, n, px, py), "nodata": np.nan,
    }
    with MemoryFile() as mem:
        with mem.open(**profile) as d:
            d.write(a, 1)
        return mem.read()


def fetch_stac(src, bbox, out_dir, quiet):
    """swisstopo publishes per-kilometre GeoTIFFs over STAC, so the tiles are fetched as published."""
    url = f"{src.stac}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}&limit=100"
    assets, seen = [], set()
    while url:
        page = json.loads(get(url))
        for f in page["features"]:
            for k, v in f["assets"].items():
                if k.endswith(".tif") and f"_{src.gsd}_" in k and k not in seen:
                    seen.add(k)
                    assets.append((k, v["href"]))
        url = next((l["href"] for l in page.get("links", []) if l.get("rel") == "next"), None)
    paths = []
    for i, (name, href) in enumerate(sorted(assets), 1):
        p = os.path.join(out_dir, "_src_" + name)
        if not os.path.exists(p):
            with open(p, "wb") as f:
                f.write(get(href))
        if not quiet:
            print(f"  [{i}/{len(assets)}] {name}")
        paths.append(p)
    return paths


FETCHERS = {"arcgis": fetch_arcgis, "wcs20": fetch_wcs20, "wcs10": fetch_wcs10, "wms_bil": fetch_wms_bil}


def to_wgs84(paths, out_path):
    """Merge whatever came back and reproject it once, because the baker wants WGS84 rasters."""
    held, srcs = [], []
    for path in paths:
        src = rasterio.open(path)
        if src.dtypes[0] == "float32" and src.nodata is not None and abs(src.nodata) < 1e30:
            srcs.append(src)
            continue
        # Voids arrive as an integer sentinel, as the float maximum, or undeclared, and `merge`
        # drops a nodata it cannot cast. NaN is a void in every type, and `obc-dem` reads it.
        a = src.read(1).astype("float32")
        a[(a == src.nodata) if src.nodata is not None else (np.abs(a) > 1e30)] = np.nan
        mem = MemoryFile()
        with mem.open(**{**src.meta, "dtype": "float32", "nodata": np.nan, "count": 1}) as d:
            d.write(a, 1)
        held.append(mem)
        srcs.append(mem.open())
        src.close()

    # A reprojected box leaves empty corners, so the void has to survive the warp as well.
    void = srcs[0].nodata
    arr, tr = merge(srcs, nodata=void, dtype="float32")
    meta = srcs[0].meta.copy()
    meta.update(height=arr.shape[1], width=arr.shape[2], transform=tr, count=1, dtype="float32")
    bounds = rasterio.transform.array_bounds(meta["height"], meta["width"], tr)
    t, w, h = calculate_default_transform(meta["crs"], "EPSG:4326", meta["width"], meta["height"], *bounds)
    dst = meta.copy()
    dst.update(crs="EPSG:4326", transform=t, width=w, height=h, compress="deflate")
    with rasterio.open(out_path, "w", **dst) as out:
        reproject(source=arr[0], destination=rasterio.band(out, 1), src_transform=tr,
                  src_crs=meta["crs"], dst_transform=t, dst_crs="EPSG:4326",
                  resampling=Resampling.bilinear, src_nodata=void, dst_nodata=void)
    for s in srcs:
        s.close()
    for m in held:
        m.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--list", action="store_true", help="print the registry and exit")
    ap.add_argument("--source", help="registry key, or `auto` to pick by the box")
    ap.add_argument("--bbox", help="min_lon,min_lat,max_lon,max_lat (longitude first, like obc-pack)")
    ap.add_argument("--out", help="output directory, handed to `obc-dem surface --reference`")
    ap.add_argument("--resolution", type=float, help="output metres per pixel (default: the source's own)")
    ap.add_argument("--quiet", action="store_true")
    # A western longitude starts with a minus, which argparse reads as the next option.
    argv = iter(sys.argv[1:])
    args = ap.parse_args([f"--bbox={next(argv, '')}" if a == "--bbox" else a for a in argv])

    if args.list or not (args.source and args.bbox and args.out):
        print(f"{'key':7s} {'country':16s} {'res':>5s}  {'licence':32s} source")
        for s in SOURCES:
            print(f"{s.key:7s} {s.country:16s} {s.resolution_m:5.1f}  {s.licence:32s} {s.name}")
        print("\nEvery source above answers without a key. See README.md for the ones that do not.")
        return 0 if args.list else 2

    bbox = tuple(float(v) for v in args.bbox.split(","))
    if len(bbox) != 4 or bbox[0] >= bbox[2] or bbox[1] >= bbox[3]:
        return err("--bbox is min_lon,min_lat,max_lon,max_lat")
    if args.source == "auto":
        matches = [s for s in SOURCES if s.covers(bbox)]
        if not matches:
            return err("no registry source covers that box; the bake falls back to Copernicus alone")
        src = min(matches, key=lambda s: s.resolution_m)
        print(f"auto: {src.country} — {src.name}")
    elif args.source in BY_KEY:
        src = BY_KEY[args.source]
    else:
        return err(f"unknown source `{args.source}`; --list shows the registry")
    if not src.covers(bbox):
        print(f"warning: {bbox} looks outside {src.country}", file=sys.stderr)

    os.makedirs(args.out, exist_ok=True)
    resolution = args.resolution or src.resolution_m
    lat = (bbox[1] + bbox[3]) / 2
    out_path = os.path.join(args.out, f"reference_{src.key}.tif")

    if src.kind == "stac":
        paths = fetch_stac(src, bbox, args.out, args.quiet)
        if not paths:
            return err("the service returned no tiles for that box")
        to_wgs84(paths, out_path)
        for p in paths:
            os.remove(p)
    else:
        parts, boxes = [], list(tiles(bbox, resolution, lat))
        for i, tile in enumerate(boxes, 1):
            if not args.quiet:
                print(f"  [{i}/{len(boxes)}] {tile[0]:.4f},{tile[1]:.4f} → {tile[2]:.4f},{tile[3]:.4f}")
            data = FETCHERS[src.kind](src, tile, resolution, lat)
            p = os.path.join(args.out, f"_src_{i}.tif")
            with open(p, "wb") as f:
                f.write(data)
            parts.append(p)
        to_wgs84(parts, out_path)
        for p in parts:
            os.remove(p)

    with rasterio.open(out_path) as d:
        a = d.read(1, masked=True)
        step = abs(d.transform.a) * 111320.0 * math.cos(math.radians(lat))
        print(f"{out_path}: {d.width}x{d.height}, {step:.1f} m/px, {a.min():.0f}..{a.max():.0f} m")
    print(f"\nAttribution: {src.attribution} ({src.licence})")
    return 0


def err(message):
    print(f"fetch_reference: {message}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
