"""The ground sources of the TrailView material and lithology: `ground.json` of data/trailview/, as
specs/trailview-tiles.md defines it. A source is read by the reader of its protocol, its native classes
become codes through its legend, and its combine rule joins them into the layer.

`fetch` is the capture of a source whose protocol is `arcgis-image`, `arcgis-features` or `wfs`: the
files of the service over a box.
"""

import base64
import io
import json
import math
import re
import sys
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import NamedTuple

import numpy as np

from . import colour, tiles

GROUND = Path(__file__).resolve().parents[2] / "data/trailview/ground.json"
# The readers of the steps. A capture writes vector files (`arcgis-features`, `wfs`) or map images.
PROTOCOLS = ("raster", "arcgis-image", "vector", "arcgis-features", "wfs", "osm")
CAPTURED = ("arcgis-image", "arcgis-features", "wfs")
VECTOR = ("vector", "arcgis-features", "wfs")
MATCHES = ("code", "prefix", "word", "tags")
COMBINES = ("authority", "refine")
# A code is one byte.
CODES = 256
# Map images at most this many pixels on a side; feature requests of this many ids or features.
IMAGE_MAX_PX = 4096
FEATURE_BATCH = 200
WFS_PAGE = 5000


class Tile(NamedTuple):
    """A tile that a chain bakes: its EPSG:3857 transform and texel size in metres; for the material, the
    quarters of the satellite, the year's highest NDVI codes and the lithology."""

    transform: object
    texel: float
    quarters: dict | None
    most: np.ndarray | None
    rock: np.ndarray | None


def mercator_m(lon, lat):
    """EPSG:3857 metres of degrees."""
    r = tiles.EARTH_M / (2 * math.pi)
    lat = np.clip(np.asarray(lat, float), -85.06, 85.06)
    return r * np.radians(np.asarray(lon, float)), r * np.log(np.tan(np.pi / 4 + np.radians(lat) / 2))


def spread(index, lost, otherwise, avoid=None, steps=None):
    """Lost texels take the value of the nearest texel that is neither lost nor in `avoid`, spreading
    in from their edges; those it does not reach within `steps` take `otherwise`."""
    index = index.copy()
    source = ~lost if avoid is None else ~lost & ~avoid
    open_ = lost.copy()
    step = 0
    while open_.any() and step != steps:
        step += 1
        filled = False
        for axis, d in ((0, 1), (0, -1), (1, 1), (1, -1)):
            near, near_source = np.zeros_like(index), np.zeros_like(source)
            to, src = (slice(1, None), slice(None, -1)) if d > 0 else (slice(None, -1), slice(1, None))
            if axis == 0:
                near[to], near_source[to] = index[src], source[src]
            else:
                near[:, to], near_source[:, to] = index[:, src], source[:, src]
            fill = open_ & near_source
            index[fill] = near[fill]
            source |= fill
            open_ &= ~fill
            filled |= bool(fill.any())
        if not filled:
            break
    index[open_] = otherwise
    return index


def box_mean(a, r):
    """The mean over a (2r + 1)² box, edges repeated."""
    n = 2 * r + 1
    s = np.pad(np.pad(a, r, mode="edge").cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    return (s[n:, n:] - s[:-n, n:] - s[n:, :-n] + s[:-n, :-n]) / (n * n)


class Vocabulary:
    """The codes of `materials.json`."""

    def __init__(self, vocabulary):
        self.material = {entry["id"]: code for code, entry in enumerate(vocabulary["materials"])}
        self.lithology = {entry["id"]: code for code, entry in enumerate(vocabulary["lithologies"])}
        if len(self.material) > 64:
            raise ValueError(f"{len(self.material)} materials do not fit in the 6 bits of the material byte")
        self.water = self.material["inland-water"]

    def round_shores(self, material, px_m, texel):
        """Finer than a raster's pixels, its shores are stair-stepped: water by the majority within
        half a pixel instead, and land that loses its water takes the nearest land's material."""
        r = round(px_m / 2 / texel)
        if not r:
            return material
        material = material.copy()
        water = box_mean((material == self.water).astype(np.float32), r) > 0.5
        lost = (material == self.water) & ~water
        material[water] = self.water
        return spread(material, lost, self.material["grassland"], avoid=(material == self.material["none"]) | water)


class Legend:
    """A source's native classes as codes of its layer, 0 for none. `code` matches the whole value, or
    the last part of a URI; `prefix` the longest class that starts the value; `word` the class found
    earliest in the text, ignoring case, the longest on a tie; `tags` the first class whose tags an OSM
    area has."""

    def __init__(self, legend, codes):
        self.match = legend["match"]
        self.classes = [(native, codes[name]) for native, name in legend["classes"]]
        self.exact = {native: code for native, code in self.classes} if self.match == "code" else {}

    def __call__(self, value):
        if value is None:
            return 0
        if self.match == "tags":
            return next((code for tags, code in self.classes if all(value.get(k) == v for k, v in tags.items())), 0)
        value = str(value)
        if self.match == "code":
            return self.exact.get(value) or self.exact.get(value.rsplit("/", 1)[-1], 0)
        if self.match == "prefix":
            found = [(len(native), code) for native, code in self.classes if value.startswith(native)]
        else:
            lower = value.lower()
            found = [(-lower.find(native), len(native), code) for native, code in self.classes if native in lower]
        return max(found)[-1] if found else 0

    def table(self):
        """The codes of the byte values 0 to 255."""
        return np.array([self(value) for value in range(256)], np.uint8)


def combine(out, proposed, rule, fine, keep=()):
    """The layer `out` with the codes that `proposed` gives by `rule`: `authority` takes each code but
    0 and leaves the codes `keep` as they are; `refine` takes a code where `fine[code, out]`, where it
    names the texel more precisely."""
    at = fine[proposed, out] if rule == "refine" else (proposed != 0) & ~np.isin(out, keep)
    out = out.copy()
    out[at] = proposed[at]
    return out


def refinements(refines, codes):
    """`fine[code, coarser]`: whether the code names the coarser code more precisely."""
    fine = np.zeros((CODES, CODES), bool)
    for name, coarser in refines.items():
        fine[codes[name], [codes[c] for c in coarser]] = True
    return fine


class Polygons:
    """Polygons with a code, burnt onto tiles: the smaller over the larger, 0 under none."""

    def __init__(self, polygons):
        import shapely

        self.geometries = [geometry for geometry, _ in polygons]
        self.codes = [code for _, code in polygons]
        self.areas = shapely.area(self.geometries) if polygons else []
        self.tree = shapely.STRtree(self.geometries)

    def burn(self, transform, size=tiles.TILE):
        import shapely
        from rasterio.features import rasterize

        x0, y0 = transform.c, transform.f
        box = shapely.box(x0, y0 + transform.e * size, x0 + transform.a * size, y0)
        found = sorted(self.tree.query(box), key=lambda i: -self.areas[i])
        if not found:
            return np.zeros((size, size), np.uint8)
        return rasterize([(self.geometries[i], self.codes[i]) for i in found], out_shape=(size, size), transform=transform,
                         dtype=np.uint8)

    def __call__(self, tile):
        return self.burn(tile.transform)

    def near(self, tile, reach_m):
        """The codes, and where there are none, the nearest code within `reach_m`."""
        from affine import Affine

        pad = round(reach_m / tile.texel)
        t = tile.transform
        codes = self.burn(Affine(t.a, 0, t.c - pad * t.a, 0, t.e, t.f - pad * t.e), tiles.TILE + 2 * pad)
        return spread(codes, codes == 0, 0, steps=pad)[pad:pad + tiles.TILE, pad:pad + tiles.TILE]


def vector_polygons(files, entry, bounds, legend):
    """The units of a vector source over the bounds that its legend names, in EPSG:3857. A file is a
    GeoPackage, a zipped Shapefile or GeoJSON, by its name; a store object has no extension."""
    import pyogrio
    import shapely
    from pyproj import Transformer

    out, field, layer = [], entry["field"], entry.get("layer")
    for name, path in sorted(files.items()):
        if name.endswith(".zip"):
            path, layer_of = shapefile(path, layer), None
        elif name.endswith((".gpkg", ".geojson")):
            path, layer_of = str(path), layer if name.endswith(".gpkg") else None
        else:
            continue
        crs = pyogrio.read_info(path, layer=layer_of)["crs"]
        to_map = Transformer.from_crs("EPSG:4326", crs, always_xy=True)
        to_tiles = Transformer.from_crs(crs, "EPSG:3857", always_xy=True)
        _, _, wkb, (values,) = pyogrio.raw.read(path, layer=layer_of, columns=[field],
                                                bbox=to_map.transform_bounds(*bounds, densify_pts=21))
        codes = {}
        for geometry, value in zip(shapely.from_wkb(wkb), values):
            code = codes[value] if value in codes else codes.setdefault(value, legend(value))
            if code and geometry is not None:
                out.append((shapely.transform(geometry, lambda xy: np.stack(to_tiles.transform(xy[:, 0], xy[:, 1]), axis=-1)), code))
    return Polygons(out)


def shapefile(archive, layer):
    """The GDAL path of the Shapefile `layer` in a zip archive, read in place. The braces let GDAL open
    an archive whose name, as a store object's, has no `.zip`."""
    import zipfile

    with zipfile.ZipFile(archive) as z:
        member = next(name for name in z.namelist() if name == f"{layer}.shp" or name.endswith(f"/{layer}.shp"))
    return f"/vsizip/{{{archive}}}/{member}"


def osm_polygons(path, legends):
    """The OSM areas of the file that each legend names: one Polygons per legend."""
    import osmium
    import shapely

    found = [[] for _ in legends]
    keys = sorted({key for legend in legends for tags, _ in legend.classes for key in tags})
    factory = osmium.geom.WKBFactory()
    # A store object has no file extension that names its format.
    osm = osmium.io.File(str(path), "pbf")
    for area in osmium.FileProcessor(osm).with_areas(osmium.filter.KeyFilter(*keys)).with_filter(
            osmium.filter.EntityFilter(osmium.osm.AREA)):
        tags = dict(area.tags)
        codes = [legend(tags) for legend in legends]
        if not any(codes):
            continue
        try:
            geometry = shapely.from_wkb(factory.create_multipolygon(area))
        except RuntimeError:
            continue
        geometry = shapely.transform(geometry, lambda xy: np.stack(mercator_m(xy[:, 0], xy[:, 1]), axis=-1))
        for code, out in zip(codes, found):
            if code:
                out.append((geometry, code))
    return [Polygons(polygons) for polygons in found]


def raster(paths, transform, size=tiles.TILE):
    """The values of the rasters on the EPSG:3857 grid `transform`, nearest; 0 where they have none."""
    import rasterio
    from rasterio.warp import Resampling, reproject

    out = np.zeros((size, size), np.uint8)
    for path in paths:
        with rasterio.open(path) as src:
            reproject(rasterio.band(src, 1), out, dst_transform=transform, dst_crs="EPSG:3857", resampling=Resampling.nearest,
                      src_nodata=0, dst_nodata=0, init_dest_nodata=False)
    return out


class Raster:
    """A raster source: its legend's codes, and with `px_m`, its shores rounded."""

    def __init__(self, entry, files, legend, vocabulary):
        self.paths, self.table, self.px_m, self.vocabulary = sorted(files.values()), legend.table(), entry.get("px_m"), vocabulary

    def __call__(self, tile):
        codes = self.table[raster(self.paths, tile.transform)]
        return self.vocabulary.round_shores(codes, self.px_m, tile.texel) if self.px_m else codes


def image_legend(legend, layer):
    """The legend of layer `layer` of an ArcGIS map service as 0xRRGGBB → class, from its swatches."""
    from PIL import Image

    colours = {}
    for entry in next(found for found in legend["layers"] if str(found["layerId"]) == layer)["legend"]:
        swatch = np.asarray(Image.open(io.BytesIO(base64.b64decode(entry["imageData"]))).convert("RGB"))
        r, g, b = (int(v) for v in swatch[swatch.shape[0] // 2, swatch.shape[1] // 2])
        colours[(r << 16) | (g << 8) | b] = entry["label"].split(":")[0].strip()
    return colours


def map_images(files, layer, legend):
    """[(codes, transform)] of each map image of an `arcgis-image` capture, named by its box in whole
    metres in the map's CRS."""
    from affine import Affine
    from PIL import Image as Picture

    lookup = {rgb: code for rgb, value in image_legend(json.loads(files["legend.json"].read_bytes()), layer).items()
              if (code := legend(value))}
    images = []
    for name, path in sorted(files.items()):
        if not (found := re.search(r"(-?\d+)_(-?\d+)_(-?\d+)_(-?\d+)\.png$", name)):
            continue
        west, south, east, north = map(int, found.groups())
        rgba = np.asarray(Picture.open(path).convert("RGBA")).astype(np.int64)
        rgb = np.where(rgba[..., 3] > 0, (rgba[..., 0] << 16) | (rgba[..., 1] << 8) | rgba[..., 2], -1)
        codes = np.zeros(rgb.shape, np.uint8)
        for value, code in lookup.items():
            codes[rgb == value] = code
        height, width = codes.shape
        images.append((codes, Affine((east - west) / width, 0, west, 0, -(north - south) / height, north)))
    return images


class Image:
    """Map images [(codes, transform)] in `crs` on a tile, nearest."""

    def __init__(self, crs, images):
        self.crs, self.images = crs, images

    def __call__(self, tile):
        from rasterio.warp import Resampling, reproject

        out = np.zeros((tiles.TILE, tiles.TILE), np.uint8)
        for array, source in self.images:
            reproject(array, out, src_transform=source, src_crs=self.crs, dst_transform=tile.transform, dst_crs="EPSG:3857",
                      resampling=Resampling.nearest, src_nodata=0, dst_nodata=0, init_dest_nodata=False)
        return out


class Map:
    """One source of the chain: its codes on a tile joined by its combine rule. With `reach_m`, where
    it names nothing, its nearest code within that reach refines the texel."""

    def __init__(self, propose, rule, fine, keep=(), reach_m=0):
        self.propose, self.rule, self.fine, self.keep, self.reach_m = propose, rule, fine, keep, reach_m

    def __call__(self, out, tile):
        out = combine(out, self.propose(tile), self.rule, self.fine, self.keep)
        return combine(out, self.propose.near(tile, self.reach_m), "refine", self.fine) if self.reach_m else out


class Heath:
    """The rule `satellite-heath`: heath where, in summer's quarter or spring's where summer saw nothing,
    the ground reflects as much red as green while its NDVI is high and near the year's highest; on
    calcareous rock (`spring_on`), only where spring saw no green grass or snow hid it."""

    def __init__(self, entry, vocabulary):
        self.entry, self.code = entry, vocabulary.material[entry["material"]]
        self.on = [vocabulary.material[name] for name in entry["on"]]
        self.lime = [vocabulary.lithology[name] for name in entry["spring_on"]]

    def __call__(self, out, tile):
        e, q = self.entry, tile.quarters
        texels = np.where((q[3][..., 3] > 0)[..., None], q[3], q[2])
        refl, spring = (texels[..., :3] / 255.0) ** 2.2, (q[2][..., :3] / 255.0) ** 2.2
        ndvi = (texels[..., 3] - 1.0) / 127 - 1
        heath = ((texels[..., 3] > 0) & (refl[..., 1] < e["green_red"] * refl[..., 0]) & (ndvi >= e["ndvi"])
                 & (ndvi >= (tile.most - 1.0) / 127 - 1 - e["peak"])
                 & ((q[2][..., 3] <= colour.SNOW_MARK) | (spring[..., 1] < e["spring_green_red"] * spring[..., 0])
                    | ~np.isin(tile.rock, self.lime))
                 & np.isin(out, self.on))
        out = out.copy()
        out[heath] = self.code
        return out


def chain(layer, ground, vocabulary, files, bounds, osm=None):
    """The steps of `layer` (`material` or `lithology`) for the sources that the step reads, and their
    zooms. `files(source)` gives the files of a source, None for one it does not read; `osm` is the
    OSM file. A material's authority keeps the water."""
    spec, codes = ground[layer], getattr(vocabulary, layer)
    fine, keep = refinements(spec["refines"], codes), [vocabulary.water] if layer == "material" else []
    entries = [entry for entry in spec["sources"] if (osm if entry.get("protocol") == "osm" else files(entry["source"]) is not None)]
    on_osm = [Legend(entry["legend"], codes) for entry in entries if entry.get("protocol") == "osm"]
    burnt = iter(osm_polygons(osm, on_osm) if on_osm else [])
    steps = []
    for entry in entries:
        if entry.get("rule") == "satellite-heath":
            steps.append(Heath(entry, vocabulary))
            continue
        legend, protocol, found = Legend(entry["legend"], codes), entry["protocol"], files(entry["source"])
        if protocol == "osm":
            propose = next(burnt)
        elif protocol == "raster":
            propose = Raster(entry, found, legend, vocabulary)
        elif protocol == "arcgis-image":
            propose = Image(entry["crs"], map_images(found, entry["layer"], legend))
        else:
            propose = vector_polygons(found, entry, bounds, legend)
        steps.append(Map(propose, entry["combine"], fine, keep, entry.get("reach_m", 0)))
    return steps, [entry["zoom"] for entry in entries if "zoom" in entry]


def entry_of(source, ground):
    """The entry of `source` with a captured protocol."""
    for layer in ("material", "lithology"):
        for entry in ground[layer]["sources"]:
            if entry["source"] == source and entry.get("protocol") in CAPTURED:
                return entry
    raise ValueError(f"{source} is no captured source of {GROUND.name}")


def fetch(source, bounds, out, zoom, ground=None):
    """The capture of `source` over the bounds into `out`. Vector features are simplified by a quarter
    of a texel at the equator of the source's `zoom`, else of `zoom`: less than a texel shows."""
    from .. import data_registry

    ground = ground or json.loads(GROUND.read_text())
    entry = entry_of(source, ground)
    url = data_registry.SOURCES[source]["fetch"]["url"]
    out.mkdir(parents=True, exist_ok=True)
    tolerance = 360 / ((1 << entry.get("zoom", zoom)) * tiles.TILE) / 4
    {"arcgis-image": fetch_image, "arcgis-features": fetch_features, "wfs": fetch_wfs}[entry["protocol"]](
        url, entry, bounds, out, tolerance)


def get(url, params=None, data=None, timeout=300):
    """The body of the answer to a GET with `params`, or to a POST of the form `data`."""
    from urllib.request import Request

    from .. import planner_runtime as runtime

    query = f"?{urllib.parse.urlencode(params)}" if params else ""
    body = urllib.parse.urlencode(data).encode() if data else None
    with runtime.open_url(Request(url + query, data=body), timeout=timeout) as response:
        return response.read()


def write(target, body):
    """Write a file of a capture whole, so that a resumed capture skips only whole files."""
    part = target.with_name(target.name + ".part")
    part.write_bytes(body)
    part.replace(target)
    print(f"{target.parent.name} {target.name}", file=sys.stderr, flush=True)


def fetch_image(url, entry, bounds, out, _):
    """The legend, and the map images of layer `layer` that cover the bounds in `crs` at `px_m`, at
    most IMAGE_MAX_PX on a side, each named W_S_E_N.png by its box in whole metres."""
    from pyproj import Transformer

    (out / "legend.json").write_bytes(get(f"{url}/legend", {"f": "json"}))
    px, epsg = entry["px_m"], entry["crs"].split(":")[1]
    west, south, east, north = Transformer.from_crs("EPSG:4326", entry["crs"], always_xy=True).transform_bounds(*bounds, densify_pts=21)
    snap = lambda v, rounding: int(rounding(v / px) * px)
    west, south, east, north = snap(west, math.floor), snap(south, math.floor), snap(east, math.ceil), snap(north, math.ceil)
    step_m = int(px * IMAGE_MAX_PX)
    for x in range(west, east, step_m):
        for y in range(south, north, step_m):
            box = (x, y, min(x + step_m, east), min(y + step_m, north))
            target = out / f"{box[0]}_{box[1]}_{box[2]}_{box[3]}.png"
            if not target.exists():
                write(target, get(f"{url}/export", {
                    "bbox": ",".join(map(str, box)), "bboxSR": epsg, "imageSR": epsg, "layers": f"show:{entry['layer']}",
                    "format": "png32", "size": f"{round((box[2] - box[0]) / px)},{round((box[3] - box[1]) / px)}",
                    "transparent": "true", "f": "image"}))


def arcgis(body):
    """The JSON of an ArcGIS answer, which reports an error with status 200."""
    found = json.loads(body)
    if "error" in found:
        raise RuntimeError(f"ArcGIS: {found['error']}")
    return found


def fetch_features(url, entry, bounds, out, tolerance):
    """The features of the layer at `url` that meet the bounds, as GeoJSON in EPSG:4326 with the field
    `field`: `features-NNNNN.geojson`, FEATURE_BATCH by object id each, three requests at a time."""
    west, south, east, north = bounds
    ids = arcgis(get(f"{url}/query", data={
        "where": "1=1", "geometry": f"{west},{south},{east},{north}", "geometryType": "esriGeometryEnvelope", "inSR": 4326,
        "spatialRel": "esriSpatialRelIntersects", "returnIdsOnly": "true", "f": "json"}))["objectIds"] or []
    ids = sorted(ids)

    def batch(i):
        target = out / f"features-{i // FEATURE_BATCH:05}.geojson"
        if not target.exists():
            body = get(f"{url}/query", data={
                "objectIds": ",".join(map(str, ids[i:i + FEATURE_BATCH])), "outFields": entry["field"], "outSR": 4326,
                "geometryPrecision": 6, "maxAllowableOffset": f"{tolerance:.8f}", "f": "geojson"})
            arcgis(body)
            write(target, body)

    with ThreadPoolExecutor(3) as pool:
        list(pool.map(batch, range(0, len(ids), FEATURE_BATCH)))


def fetch_wfs(url, entry, bounds, out, _):
    """The features of the WFS 2.0 type `layer` that meet the bounds, as GeoJSON in EPSG:4326 with only
    the field `field`: `features-NNNNN.geojson`, WFS_PAGE each."""
    west, south, east, north = bounds
    field, page = entry["field"], 0
    while True:
        target = out / f"features-{page:05}.geojson"
        if target.exists():
            features = json.loads(target.read_bytes())["features"]
        else:
            features = json.loads(get(url, {
                "service": "WFS", "version": "2.0.0", "request": "GetFeature", "typeNames": entry["layer"],
                "outputFormat": "application/json", "srsName": "EPSG:4326", "count": WFS_PAGE, "startIndex": page * WFS_PAGE,
                "bbox": f"{south},{west},{north},{east},urn:ogc:def:crs:EPSG::4326"}))["features"]
            features = [{"type": "Feature", "geometry": f["geometry"], "properties": {field: f["properties"].get(field)}}
                        for f in features]
            if features:
                write(target, json.dumps({"type": "FeatureCollection", "features": features}).encode())
        if len(features) < WFS_PAGE:
            return
        page += 1
