"""The ground sources of the TrailView material and lithology: `ground.json` of data/trailview/, as
specs/trailview-tiles.md defines it. A source is read by the reader of its protocol, its native classes
become codes through its legend, and its combine rule joins them into the layer.

`fetch` is the capture of a source whose protocol is `arcgis-features` or `archive-features`: the
features of a service, or of a file of the archive, over a box.
"""

import collections
import hashlib
import itertools
import json
import math
import os
import sys
import tempfile
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import NamedTuple

import numpy as np

from . import colour, tiles

GROUND = Path(__file__).resolve().parents[2] / "data/trailview/ground.json"
# The readers of the steps. A capture writes vector files over a box.
PROTOCOLS = ("raster", "vector", "arcgis-features", "archive-features", "osm")
CAPTURED = ("arcgis-features", "archive-features")
VECTOR = ("vector", *CAPTURED)
MATCHES = ("code", "prefix", "word", "tags")
COMBINES = ("authority", "refine")
# A code is one byte.
CODES = 256
# Feature requests of this many ids or features.
FEATURE_BATCH = 200
# Features per page of an archive capture. Chosen, not measured: a page of a national map stays in memory.
ARCHIVE_PAGE = 5000
# The space that an unpacked copy leaves free on the disk of the store: `RESERVE` of host/obc-data/src/store.rs.
RESERVE = 4 << 30
TRIES = 6
# The share of the listed ids that may vanish before an ArcGIS capture fetches them.
VANISHED = 0.001
# Features per read of a vector file while the step keeps its units.
UNITS_CHUNK = 50_000


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
    area has. A `word` legend's `qualifier` names its own class where its word follows the found word
    within `within` characters, or stands in a text without a class."""

    def __init__(self, legend, codes):
        self.match = legend["match"]
        self.classes = [(native, codes[name]) for native, name in legend["classes"]]
        self.exact = {native: code for native, code in self.classes} if self.match == "code" else {}
        self.qualifier = legend.get("qualifier")
        if self.qualifier:
            self.qualifier = {**self.qualifier, "code": codes[self.qualifier["class"]]}

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
            return max(found)[-1] if found else 0
        lower = value.lower()
        found = min(((lower.find(native), -len(native), code) for native, code in self.classes if native in lower), default=None)
        if q := self.qualifier:
            at = lower.find(q["word"])
            if at >= 0 and (found is None or 0 <= at - found[0] + found[1] <= q["within"]):
                return q["code"]
        return found[2] if found else 0

    def table(self, values=None):
        """The codes of the byte values 0 to 255, each the native class that `values` names for it."""
        return np.array([self(value if values is None else values.get(str(value))) for value in range(256)], np.uint8)

    def where(self, field):
        """An ArcGIS `where` that keeps the features whose `field` may have a class."""
        quote = lambda text: "'" + text.replace("'", "''") + "'"
        if self.match == "code":
            terms = [f"{field} = {quote(native)} OR {field} LIKE {quote('%/' + native)}" for native, _ in self.classes]
        elif self.match == "prefix":
            terms = [f"{field} LIKE {quote(native + '%')}" for native, _ in self.classes]
        else:
            words = [native for native, _ in self.classes] + ([self.qualifier["word"]] if self.qualifier else [])
            terms = [f"LOWER({field}) LIKE {quote('%' + word + '%')}" for word in words]
        return " OR ".join(terms)


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


class Burn:
    """Polygons with a code, burnt onto tiles: the smaller over the larger, 0 under none. `within(box)`
    gives the geometries, codes and areas that meet an EPSG:3857 box."""

    def burn(self, transform, size=tiles.TILE):
        from rasterio.features import rasterize

        x0, y0 = transform.c, transform.f
        geometries, codes, areas = self.within((x0, y0 + transform.e * size, x0 + transform.a * size, y0))
        if not len(codes):
            return np.zeros((size, size), np.uint8)
        order = np.argsort(-np.asarray(areas), kind="stable")
        return rasterize([(geometries[i], codes[i]) for i in order], out_shape=(size, size), transform=transform, dtype=np.uint8)

    def __call__(self, tile):
        return self.burn(tile.transform)

    def near(self, tile, reach_m):
        """The codes, and where there are none, the nearest code within `reach_m`."""
        from affine import Affine

        pad = round(reach_m / tile.texel)
        t = tile.transform
        codes = self.burn(Affine(t.a, 0, t.c - pad * t.a, 0, t.e, t.f - pad * t.e), tiles.TILE + 2 * pad)
        return spread(codes, codes == 0, 0, steps=pad)[pad:pad + tiles.TILE, pad:pad + tiles.TILE]


class Polygons(Burn):
    """Polygons [(geometry in EPSG:3857, code)] in memory."""

    def __init__(self, polygons):
        import shapely

        self.geometries = [geometry for geometry, _ in polygons]
        self.codes = [code for _, code in polygons]
        self.areas = shapely.area(self.geometries) if polygons else []
        self.tree = shapely.STRtree(self.geometries)

    def within(self, box):
        import shapely

        found = self.tree.query(shapely.box(*box))
        return [self.geometries[i] for i in found], [self.codes[i] for i in found], [self.areas[i] for i in found]


class Units(Burn):
    """The units of a vector source over the bounds that its legend names, kept in the GeoPackage `path`
    in EPSG:3857 with their code and area, and read per tile: a national map never fits in memory. A
    file is a GeoPackage, a zipped Shapefile or GeoJSON, by its name; a store object has no extension."""

    def __init__(self, files, entry, bounds, legend, path):
        import pyogrio
        import shapely
        from pyproj import Transformer

        self.path, self.count = str(path), 0
        field, layer, transformers = entry["field"], entry.get("layer"), {}
        for name, source in sorted(files.items()):
            if name.endswith(".zip"):
                source, layer_of = shapefile(source, layer), None
            elif name.endswith((".gpkg", ".geojson")):
                source, layer_of = str(source), layer if name.endswith(".gpkg") else None
            else:
                continue
            crs = pyogrio.read_info(source, layer=layer_of)["crs"]
            if crs not in transformers:
                to_map = Transformer.from_crs("EPSG:4326", crs, always_xy=True)
                transformers[crs] = (to_map.transform_bounds(*bounds, densify_pts=21),
                                     Transformer.from_crs(crs, "EPSG:3857", always_xy=True))
            box, to_tiles = transformers[crs]
            names = {}
            for skip in itertools.count(0, UNITS_CHUNK):
                _, _, wkb, (values,) = pyogrio.raw.read(source, layer=layer_of, columns=[field], bbox=box,
                                                        skip_features=skip, max_features=UNITS_CHUNK)
                codes = np.array([names[v] if v in names else names.setdefault(v, legend(v)) for v in values], np.int32)
                geometries = shapely.from_wkb(wkb)
                keep = (codes > 0) & ~shapely.is_missing(geometries)
                if keep.any():
                    kept = shapely.transform(geometries[keep], lambda xy: np.stack(to_tiles.transform(xy[:, 0], xy[:, 1]), axis=-1))
                    pyogrio.raw.write(self.path, geometry=shapely.to_wkb(kept), field_data=[codes[keep], shapely.area(kept)],
                                      fields=["code", "area"], geometry_type="Unknown", crs="EPSG:3857", driver="GPKG",
                                      layer="units", append=self.count > 0)
                    self.count += int(keep.sum())
                if len(values) < UNITS_CHUNK:
                    break

    def within(self, box):
        import pyogrio
        import shapely

        if not self.count:
            return [], [], []
        _, _, wkb, (codes, areas) = pyogrio.raw.read(self.path, layer="units", columns=["code", "area"], bbox=box)
        return shapely.from_wkb(wkb), codes, areas


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
    """A raster source: its legend's codes, and with `px_m`, its shores rounded. With `values`, a raster
    value is the native class that `values` names for it."""

    def __init__(self, entry, files, legend, vocabulary):
        self.paths, self.px_m, self.vocabulary = sorted(files.values()), entry.get("px_m"), vocabulary
        self.table = legend.table(entry.get("values"))

    def __call__(self, tile):
        codes = self.table[raster(self.paths, tile.transform)]
        return self.vocabulary.round_shores(codes, self.px_m, tile.texel) if self.px_m else codes


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
    """The steps of `layer` (`material` or `lithology`) for the sources that the step reads.
    `files(source)` gives the files of a source, None for one it does not read; `osm` is the OSM file.
    A material's authority keeps the water. The units of the vector sources go to a folder of
    `TMPDIR`, which the engine removes when the step ends."""
    spec, codes = ground[layer], getattr(vocabulary, layer)
    fine, keep = refinements(spec["refines"], codes), [vocabulary.water] if layer == "material" else []
    entries = [entry for entry in spec["sources"] if (osm if entry.get("protocol") == "osm" else files(entry["source"]) is not None)]
    on_osm = [Legend(entry["legend"], codes) for entry in entries if entry.get("protocol") == "osm"]
    burnt = iter(osm_polygons(osm, on_osm) if on_osm else [])
    steps, scratch = [], tempfile.mkdtemp(prefix="ground-")
    for entry in entries:
        if entry.get("rule") == "satellite-heath":
            steps.append(Heath(entry, vocabulary))
            continue
        legend, protocol, found = Legend(entry["legend"], codes), entry["protocol"], files(entry["source"])
        if protocol == "osm":
            propose = next(burnt)
        elif protocol == "raster":
            propose = Raster(entry, found, legend, vocabulary)
        else:
            propose = Units(found, entry, bounds, legend, Path(scratch) / f"units-{len(steps)}.gpkg")
        steps.append(Map(propose, entry["combine"], fine, keep, entry.get("reach_m", 0)))
    return steps


def entry_of(source, ground):
    """The entry of `source` with a captured protocol."""
    for layer in ("material", "lithology"):
        for entry in ground[layer]["sources"]:
            if entry["source"] == source and entry.get("protocol") in CAPTURED:
                return entry
    raise ValueError(f"{source} is no captured source of {GROUND.name}")


def fetch(source, bounds, out, zoom, ground=None, archive=None, work=None):
    """The capture of `source` over the bounds into `out`; an `archive-features` source reads the zip
    `archive` and unpacks it into `work`. Vector features are simplified by a quarter of a texel at the
    equator of `zoom`: less than a texel shows."""
    from .. import data_registry

    ground = ground or json.loads(GROUND.read_text())
    entry = entry_of(source, ground)
    out.mkdir(parents=True, exist_ok=True)
    tolerance = 360 / ((1 << zoom) * tiles.TILE) / 4
    if entry["protocol"] == "archive-features":
        fetch_archive(archive, work, entry, bounds, out, tolerance)
    else:
        url = data_registry.SOURCES[source]["fetch"]["url"]
        fetch_features(url, entry, bounds, out, tolerance, Legend(entry["legend"], collections.defaultdict(int)))


class Refused(Exception):
    """An answer that no try changes, as an ArcGIS error with a 4xx code."""


def get(url, params=None, data=None, check=None, timeout=300):
    """The body of the answer to a GET with `params`, or to a POST of the form `data`. A failed or cut
    try, a 5xx or 429 answer or a body that `check` refuses with ValueError is tried again, TRIES in
    all, after 2, 4, 8 … seconds. Another 4xx answer, or `Refused` from `check`, fails at once."""
    import http.client
    from urllib.error import HTTPError
    from urllib.request import Request

    from .. import planner_runtime as runtime

    query = f"?{urllib.parse.urlencode(params)}" if params else ""
    body = urllib.parse.urlencode(data).encode() if data else None
    for attempt in range(TRIES):
        try:
            with runtime.open_url(Request(url + query, data=body), timeout=timeout) as response:
                answer = response.read()
            if check:
                check(answer)
            return answer
        except (OSError, http.client.HTTPException, ValueError) as error:
            final = isinstance(error, HTTPError) and error.code < 500 and error.code != 429
            if final or attempt == TRIES - 1:
                raise
            print(f"{url}: {error}; again in {2 << attempt} s", file=sys.stderr, flush=True)
            time.sleep(2 << attempt)


def write(target, body):
    """Write a file of a capture whole and on disk, so that a resumed capture skips only whole files."""
    part = target.with_name(target.name + ".part")
    with open(part, "wb") as stream:
        stream.write(body)
        stream.flush()
        os.fsync(stream.fileno())
    part.replace(target)
    print(f"{target.parent.name} {target.name}", file=sys.stderr, flush=True)


def positions(out, meaning):
    """Keep the pages of a resumed capture only while `meaning`, what a page number stands for, is the
    same as when they were written; else they go."""
    marker = out / ".positions"
    if marker.exists() and marker.read_text() != meaning:
        for page in out.glob("features-*.geojson"):
            page.unlink()
    write(marker, meaning.encode())


def arcgis(body):
    """The JSON of an ArcGIS answer, which reports an error with status 200: `Refused` for a 4xx code,
    as an invalid `where`, else ValueError."""
    found = json.loads(body)
    if "error" in found:
        code = found["error"].get("code") if isinstance(found["error"], dict) else None
        raise (Refused if isinstance(code, int) and 400 <= code < 500 else ValueError)(f"ArcGIS: {found['error']}")
    return found


def fetch_features(url, entry, bounds, out, tolerance, legend):
    """The features of the layer at `url` that meet the bounds and whose `field` the legend may name, as
    GeoJSON in EPSG:4326 with that field: `features-NNNNN.geojson`, FEATURE_BATCH by object id each,
    three requests at a time. A batch that lacks ids, as past the service's transfer limit, asks again
    for those. An id that the service no longer has is left out, up to VANISHED of the ids."""
    west, south, east, north = bounds
    found = arcgis(get(f"{url}/query", data={
        "where": legend.where(entry["field"]), "geometry": f"{west},{south},{east},{north}",
        "geometryType": "esriGeometryEnvelope", "inSR": 4326, "spatialRel": "esriSpatialRelIntersects",
        "returnIdsOnly": "true", "f": "json"}, check=arcgis))
    key, ids = found["objectIdFieldName"], sorted(found["objectIds"] or [])
    positions(out, hashlib.sha256(",".join(map(str, ids)).encode()).hexdigest())
    vanished = []

    def batch(i):
        target = out / f"features-{i // FEATURE_BATCH:05}.geojson"
        if target.exists():
            return
        features, wanted = [], ids[i:i + FEATURE_BATCH]
        while wanted:
            found = arcgis(get(f"{url}/query", data={
                "objectIds": ",".join(map(str, wanted)), "outFields": f"{key},{entry['field']}", "outSR": 4326,
                "geometryPrecision": 6, "maxAllowableOffset": f"{tolerance:.8f}", "f": "geojson"}, check=arcgis))["features"]
            got = {feature["properties"][key] for feature in found} & set(wanted)
            if not got:
                vanished.extend(wanted)
                if len(vanished) > VANISHED * len(ids):
                    raise RuntimeError(f"{url}: {len(vanished)} of {len(ids)} ids are gone since the capture listed them")
                break
            features += [feature for feature in found if feature["properties"][key] in got]
            wanted = [i for i in wanted if i not in got]
        write(target, json.dumps({"type": "FeatureCollection", "features": features}).encode())

    with ThreadPoolExecutor(3) as pool:
        list(pool.map(batch, range(0, len(ids), FEATURE_BATCH)))
    if vanished:
        print(f"{url}: {len(vanished)} ids are gone since the capture listed them", file=sys.stderr, flush=True)


def fetch_archive(archive, work, entry, bounds, out, tolerance):
    """The features of the layer `layer` of the one GeoPackage or FileGDB in the zip `archive` that meet
    the bounds and whose `field` the legend may name, as GeoJSON in EPSG:4326 with that field, simplified
    by `tolerance` degrees: `features-NNNNN.geojson`, ARCHIVE_PAGE each in the order of their ids. The
    file is read from its copy in `work`: a read in a deflated member inflates it from its start."""
    import pyogrio
    import shapely
    from pyproj import Transformer

    path, layer, field = unpack(archive, work), entry["layer"], entry["field"]
    crs = pyogrio.read_info(path, layer=layer)["crs"]
    box = Transformer.from_crs("EPSG:4326", crs, always_xy=True).transform_bounds(*bounds, densify_pts=21)
    _, ids, _, (values,) = pyogrio.raw.read(path, layer=layer, columns=[field], bbox=box, read_geometry=False, return_fids=True)
    named = Legend(entry["legend"], collections.defaultdict(lambda: 1))
    ids = np.sort(ids[np.array([named(value) > 0 for value in values], bool)])
    to_degrees = Transformer.from_crs(crs, "EPSG:4326", always_xy=True)
    for page in out.glob("features-*.geojson"):
        page.unlink()
    for page, start in enumerate(range(0, len(ids), ARCHIVE_PAGE)):
        _, _, wkb, (values,) = pyogrio.raw.read(path, layer=layer, columns=[field], fids=ids[start:start + ARCHIVE_PAGE])
        degrees = shapely.transform(shapely.from_wkb(wkb), lambda xy: np.stack(to_degrees.transform(xy[:, 0], xy[:, 1]), axis=-1))
        geometries = shapely.transform(shapely.simplify(degrees, tolerance), lambda xy: np.round(xy, 6))
        write(out / f"features-{page:05}.geojson", json.dumps({"type": "FeatureCollection", "features": [
            {"type": "Feature", "geometry": json.loads(geometry), "properties": {field: value}}
            for geometry, value in zip(shapely.to_geojson(geometries).tolist(), values.tolist())]}).encode())


def unpack(archive, work):
    """The path of the one GeoPackage or FileGDB folder in the zip `archive`, unpacked into `work` once:
    `.complete` marks a whole copy, which every later capture reads without the zip. A file already
    there with its size stays, so a stopped unpack resumes."""
    import shutil
    import zipfile

    if (work / ".complete").is_file():
        (top,) = [path for path in work.iterdir() if path.name.endswith((".gpkg", ".gdb"))]
        return str(top)
    with zipfile.ZipFile(archive) as z:
        members = [m for m in z.infolist() if not m.is_dir() and m.filename.split("/")[0].endswith((".gpkg", ".gdb"))]
        tops = {member.filename.split("/")[0] for member in members}
        if len(tops) != 1 or any(".." in member.filename.split("/") for member in members):
            raise ValueError(f"{archive}: the zip holds not one GeoPackage or FileGDB but {sorted(tops)}")
        in_place = lambda member: (work / member.filename).is_file() and (work / member.filename).stat().st_size == member.file_size
        missing = [member for member in members if not in_place(member)]
        needed = sum(member.file_size for member in missing)
        disk = next(path for path in [work, *work.parents] if path.exists())
        if missing and (free := shutil.disk_usage(disk).free) < needed + RESERVE:
            raise RuntimeError(f"{disk} has {free / 2**30:.1f} GiB free; the copy of {Path(archive).name} needs "
                               f"{needed / 2**30:.1f} GiB more and keeps {RESERVE / 2**30:.0f} GiB free. Free space on "
                               f"that disk, or set OBC_DATA_STORE to a store on a disk with more space")
        for member in missing:
            target = work / member.filename
            target.parent.mkdir(parents=True, exist_ok=True)
            part = target.with_name(target.name + ".part")
            with z.open(member) as source, open(part, "wb") as stream:
                shutil.copyfileobj(source, stream, 1 << 20)
                stream.flush()
                os.fsync(stream.fileno())
            part.replace(target)
    # After every member is on disk, so a power cut leaves no damaged copy marked whole.
    write(work / ".complete", b"")
    return str(work / tops.pop())
