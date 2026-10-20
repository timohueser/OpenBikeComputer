"""Bake the TrailView ground tiles of one region, as specs/trailview-tiles.md defines them: the material
chain of the prototype's `bake.py` on Web Mercator tiles, the lithology and the region.

The `obc data` steps (`--step`) are `planner/trailview-material`, `planner/trailview-lithology` and
`planner/trailview-region`; the option `layer` names the one to bake. `--fetch-corine` is the
capture of `corine-land-cover`: the map images of the EEA service over a box, and its legend.
"""

import argparse
import base64
import io
import json
import math
import re
import sys
import urllib.parse
from pathlib import Path

import numpy as np

from . import tiles
from .. import planner_geo as geo, step_request

MATERIAL_ZOOM = 13
LITHOLOGY_ZOOM = REGION_ZOOM = 10
# The material byte: the code in the low 6 bits, then the barren bit; NO_DATA alone is outside the bake.
BARREN, NO_DATA = 0x40, 0x80
# A region byte with no region: no ecoregion while the walk runs, and no data in a tile.
REGION_NO_DATA = 255
WORLDCOVER_PX_M = 10.0
WORLDCOVER_MATERIALS = {10: "forest", 20: "scrub", 30: "grassland", 40: "arable", 50: "built-up", 60: "sparse",
                        70: "glacier", 80: "inland-water", 90: "marsh", 95: "marsh", 100: "moss-lichen"}
# The materials a coarser map may name more precisely: the same structure, or open ground of another
# name. WorldCover's 10 m keeps the structure; a 100 m map only names it. The satellite finds heath on
# open ground (MOOR_ON) and tells it from green grass and scrub, which a map's heath or bog then leaves
# as they are.
OPEN = {"grassland", "scrub", "sparse", "moss-lichen", "marsh"}
UNJUDGED = OPEN - {"grassland", "scrub"}
REFINES = {
    "needleleaf-forest": {"forest"}, "broadleaf-forest": {"forest"}, "mixed-forest": {"forest"},
    "sclerophyllous-scrub": {"scrub"}, "heath": UNJUDGED, "bog": UNJUDGED | {"heath"},
    "pasture": {"grassland"}, "rough-grass": {"grassland"}, "wet-grassland": {"grassland"}, "salt-marsh": {"marsh"},
    "rice": {"arable"}, "vineyard": {"arable"}, "orchard": {"arable"}, "olive-grove": {"arable"},
    "bare-soil": {"sparse"}, "bare-rock": {"sparse"}, "scree": {"sparse", "bare-rock"}, "sand": {"sparse"},
    "beach": {"sparse", "sand"},
}
# Not on sparse ground: a green pixel there is grass beside bare stone, as on a river's gravel bar.
MOOR_ON = OPEN - {"sparse"}
# Barren where the year's highest NDVI is below the middle of the prototype shader's ramp from 0.15 to 0.35.
BARREN_NDVI = 0.25
CLC_PX_M = 100.0
CLC_MAX_PX = 4096
CLC_SERVICE = "https://image.discomap.eea.europa.eu/arcgis/rest/services/Corine/CLC{version}_LAEA/MapServer"
CLC_MATERIALS = {131: "bare-soil", 132: "bare-soil", 213: "rice", 221: "vineyard", 222: "orchard", 223: "olive-grove",
                 231: "pasture", 311: "broadleaf-forest", 312: "needleleaf-forest", 313: "mixed-forest", 321: "rough-grass",
                 322: "heath", 323: "sclerophyllous-scrub", 331: "sand", 332: "bare-rock", 412: "bog", 421: "salt-marsh"}
LEAF = {"needleleaved": "needleleaf-forest", "broadleaved": "broadleaf-forest", "mixed": "mixed-forest"}
OSM_NATURAL = {"bare_rock": "bare-rock", "scree": "scree", "shingle": "beach", "sand": "sand", "beach": "beach",
               "heath": "heath", "grassland": "rough-grass"}
OSM_LANDUSE = {"vineyard": "vineyard", "orchard": "orchard"}
WETLANDS = {"bog": "bog", "saltmarsh": "salt-marsh", "wet_meadow": "wet-grassland"}
# The layer of each lithology map, the field that names the rock, and its terms for each lithology,
# split by "|". Any other term is unknown: surface deposits, which hide the rock that crags show, and
# undifferentiated units. EGDI names the rock first; IGME 5000 where EGDI names none.
EGDI = ("GeologicUnitView", "representativelithology_uri", {
    "limestone": "limestone|chalk|impureLimestone|carbonateSedimentaryRock|marble|travertine",
    "limestone-marl": "impureCarbonateSedimentaryRock",
    "dolomite": "dolomite|impureDolomite|dolomiticOrMagnesianSedimentaryRock",
    "marl-clay": "mudstone|claystone|siltstone|organicBearingMudstone",
    "gypsum": "gypsumOrAnhydrite",
    "sandstone": "sandstone|wacke|arenite|clasticSandstone",
    "conglomerate": "conglomerate|breccia|diamictite",
    "quartzite": "quartzite|nonClasticSiliceousSedimentaryRock|biogenicSilicaSedimentaryRock",
    "granite": "granite|granitoid|monzogranite|syenogranite|granodiorite|tonalite|diorite|quartzDiorite|dioriticRock|syenite|"
               "syeniticRock|monzonite|foidSyenitoid|foidBearingSyenite|anorthositicRock|phaneriticIgneousRock|acidicIgneousRock|"
               "orthogneiss|gneiss|paragneiss|migmatite|granulite",
    "schist-slate-shale": "shale|slate|schist|micaSchist|phyllite|phyllonite|amphibolite|foliatedMetamorphicRock|"
                          "chloriteActinoliteEpidoteMetamorphicRock|glaucophaneLawsoniteEpidoteMetamorphicRock",
    "basalt-lava": "basalt|alkaliOlivineBasalt|tholeiiticBasalt|basanite|tephrite|andesite|spilite|basicIgneousRock|"
                   "fineGrainedIgneousRock|komatiiticRock",
    "felsic-lava": "rhyolite|rhyolitoid|dacite|trachyte|trachytoid|trachyticRock|phonolite|porphyry",
    "dolerite": "gabbro|gabbroid|gabbroicRock|doleriticRock|ultramaficIgneousRock|peridotite|pyroxenite|serpentinite|eclogite",
    "tuff": "tuffBrecciaAgglomerateOrPyroclasticBreccia|ashTuffLapillistoneAndLapilliTuff|pyroclasticRock",
    "cinder-scoria": "pyroclasticMaterial|ashBrecciaBombOrBlockTephra",
})
IGME5000 = ("IGME5000_europeEPSG3034shp_geology_poly_v01", "Portr_Petr", {
    "limestone": "limestone|carbonates|carbonates, consolidated|chalk|marble|meta-limestone|meta-carbonate",
    "dolomite": "dolomite/dolostone",
    "marl-clay": "marl|marlstone|clay|claystone|mudstone|siltstone|mud|silt|meta-claystone",
    "gypsum": "gypsum|evaporites|rock salt",
    "sandstone": "sandstone|meta-sandstone|siliclastics, consolidated",
    "conglomerate": "conglomerate|tillite",
    "quartzite": "quartzite|chert",
    "granite": "granite|granite group|alkali granite|meta-granite|meta-granite group|granodiorite|meta-granodiorite|"
               "granodiorite-diorite group|diorite|tonalite|syenite|alkali syenite|syenite-monzonite group|plutonic alkaline group|"
               "meta-plutonic rock group|gneiss|migmatite/anatexite|granulite",
    "schist-slate-shale": "mica schist|schist group|phyllite|shale/slate|greenschist|blueschist|amphibolite|meta-sediment group|"
                          "miscellaneous metamorphic rock group",
    "basalt-lava": "basalt|basalt group|tholeiitic basalt|alkali basalt|meta-basalt|meta-basalt group|andesite|andesite group|"
                   "meta-andesite|tephrite, basanite|latite|theralite|essexite-theralite group|lamprophyre|volcanic alkaline group|"
                   "volcanic ultramafic group|meta-volcanic rock group",
    "felsic-lava": "rhyolite|rhyolite group|alkali rhyolite|meta-rhyolite|meta-rhyolite group|rhyodacite|meta-rhyodacite|dacite|"
                   "trachyte|phonolite",
    "dolerite": "gabbro|gabbro group|meta-gabbro|meta-gabbro group|dolerite|plutonic ultramafic group|peridotite|meta-peridotite|"
                "dunite, harzburgite, lherzolite|serpentinite|eclogite",
    "tuff": "tuff group|tuffite group|andesite tuffite|andesite agglomerate|volcano-sediments",
    "alluvium-colluvium": "sand|gravel",
})
# A texel outside every ecoregion takes the nearest within this reach, as a cliff top that the
# simplified coast leaves in the sea; the search runs at FALLBACK_ZOOM, 0.4 km texels at 47° N.
REACH_M = 25_000.0
FALLBACK_ZOOM = 7


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
        if len(self.material) > BARREN:
            raise ValueError(f"{len(self.material)} materials do not fit in the 6 bits of the material byte")
        self.worldcover = np.full(256, self.material["none"], np.uint8)
        for value, name in WORLDCOVER_MATERIALS.items():
            self.worldcover[value] = self.material[name]
        self.water = self.material["inland-water"]

    def refine(self, material, proposed):
        """Each proposed material where it names the texel's material more precisely (REFINES); 0
        proposes nothing."""
        out = material.copy()
        for name, coarser in REFINES.items():
            at = (proposed == self.material[name]) & np.isin(material, [self.material[c] for c in coarser])
            out[at] = self.material[name]
        return out

    def round_shores(self, material, texel):
        """Finer than WorldCover's pixels, its shores are stair-stepped: water by the majority within
        half a pixel instead, and land that loses its water takes the nearest land's material."""
        r = round(WORLDCOVER_PX_M / 2 / texel)
        if not r:
            return material
        material = material.copy()
        water = box_mean((material == self.water).astype(np.float32), r) > 0.5
        lost = (material == self.water) & ~water
        material[water] = self.water
        return spread(material, lost, self.material["grassland"], avoid=(material == self.material["none"]) | water)


def area_material(tags):
    """The material that an OSM area names, or None."""
    if tags.get("natural") == "wetland":
        return WETLANDS.get(tags.get("wetland"))
    return OSM_NATURAL.get(tags.get("natural")) or OSM_LANDUSE.get(tags.get("landuse"))


def forest_material(tags):
    if tags.get("landuse") == "forest" or tags.get("natural") == "wood":
        return LEAF.get(tags.get("leaf_type"))
    return None


def osm_areas(path, vocabulary):
    """The forests with a leaf type, and the other areas that may name a material, of the OSM file:
    each a list of (shapely geometry in EPSG:3857, material code)."""
    import osmium
    import shapely

    forests, areas = [], []
    keys = osmium.filter.KeyFilter("natural", "landuse")
    factory = osmium.geom.WKBFactory()
    for area in osmium.FileProcessor(str(path)).with_areas(keys).with_filter(osmium.filter.EntityFilter(osmium.osm.AREA)):
        tags = dict(area.tags)
        forest, other = forest_material(tags), area_material(tags)
        if not (forest or other):
            continue
        try:
            geometry = shapely.from_wkb(factory.create_multipolygon(area))
        except RuntimeError:
            continue
        geometry = shapely.transform(geometry, lambda xy: np.stack(mercator_m(xy[:, 0], xy[:, 1]), axis=-1))
        for found, out in ((forest, forests), (other, areas)):
            if found:
                out.append((geometry, vocabulary.material[found]))
    return forests, areas


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


def clc_legend(legend):
    """The legend of the CLC map service as 0xRRGGBB → class, from its swatches."""
    from PIL import Image

    colours = {}
    for entry in next(layer for layer in legend["layers"] if layer["layerId"] == 1)["legend"]:
        swatch = np.asarray(Image.open(io.BytesIO(base64.b64decode(entry["imageData"]))).convert("RGB"))
        r, g, b = (int(v) for v in swatch[swatch.shape[0] // 2, swatch.shape[1] // 2])
        colours[(r << 16) | (g << 8) | b] = int(entry["label"].split(":")[0])
    return colours


def clc_images(files, vocabulary):
    """[(material codes, EPSG:3035 transform)] of each map image of a `corine-land-cover` capture."""
    from affine import Affine
    from PIL import Image

    colours = clc_legend(json.loads(files["legend.json"].read_bytes()))
    lookup = {rgb: vocabulary.material[CLC_MATERIALS[code]] for rgb, code in colours.items() if code in CLC_MATERIALS}
    images = []
    for name, path in sorted(files.items()):
        if not (found := re.fullmatch(r"clc-(-?\d+)_(-?\d+)_(-?\d+)_(-?\d+)\.png", name)):
            continue
        west, south, east, north = map(int, found.groups())
        rgba = np.asarray(Image.open(path).convert("RGBA")).astype(np.int64)
        colour = np.where(rgba[..., 3] > 0, (rgba[..., 0] << 16) | (rgba[..., 1] << 8) | rgba[..., 2], -1)
        codes = np.zeros(colour.shape, np.uint8)
        for rgb, code in lookup.items():
            codes[colour == rgb] = code
        height, width = codes.shape
        images.append((codes, Affine((east - west) / width, 0, west, 0, -(north - south) / height, north)))
    return images


def onto(transform, sources, crs, resampling):
    """The byte arrays [(array, transform)] in `crs` on the EPSG:3857 grid `transform`; a later array
    fills only what an earlier one left 0."""
    from rasterio.warp import reproject

    out = np.zeros((tiles.TILE, tiles.TILE), np.uint8)
    for array, source in sources:
        reproject(array, out, src_transform=source, src_crs=crs, dst_transform=transform, dst_crs="EPSG:3857",
                  resampling=resampling, src_nodata=0, dst_nodata=0, init_dest_nodata=False)
    return out


def worldcover(paths, transform, size=tiles.TILE):
    """WorldCover's classes on the EPSG:3857 grid `transform`, nearest; 0 where it has none."""
    import rasterio
    from rasterio.warp import Resampling, reproject

    out = np.zeros((size, size), np.uint8)
    for path in paths:
        with rasterio.open(path) as src:
            reproject(rasterio.band(src, 1), out, dst_transform=transform, dst_crs="EPSG:3857", resampling=Resampling.nearest,
                      src_nodata=0, dst_nodata=0, init_dest_nodata=False)
    return out


class Material:
    """The material chain on one tile: WorldCover, then the satellite's heath, OSM's forests with a leaf
    type, CORINE, and OSM's natural and landuse areas, each refining the one before."""

    def __init__(self, vocabulary, bounds, worldcover, corine, forests, areas, mosaics):
        self.vocabulary, self.bounds, self.worldcover = vocabulary, bounds, worldcover
        self.corine, self.forests, self.areas, self.mosaics = corine, Polygons(forests), Polygons(areas), mosaics

    def __call__(self, z, x, y):
        from rasterio.warp import Resampling

        v, transform = self.vocabulary, tiles.transform(z, x, y)
        from . import colour

        material = v.round_shores(v.worldcover[worldcover(self.worldcover, transform)], tiles.texel_m(z, y))
        quarters = colour.quarters(self.mosaics, transform, tiles.texel_m(z, y))
        most = np.max([texels[..., 3] for texels in quarters.values()], axis=0)
        heath = colour.satellite_heath(quarters, most) & np.isin(material, [v.material[m] for m in MOOR_ON])
        material[heath] = v.material["heath"]
        barren = (most > colour.SNOW_MARK) & (colour.ndvi(most) < BARREN_NDVI)
        material = v.refine(material, self.forests.burn(transform))
        material = v.refine(material, onto(transform, self.corine, "EPSG:3035", Resampling.nearest))
        material = v.refine(material, self.areas.burn(transform))
        material = np.where(barren, material | BARREN, material).astype(np.uint8)
        material[~tiles.inside(self.bounds, z, x, y)] = NO_DATA
        return material


def lithology_units(paths, layers, bounds, vocabulary):
    """The units of each lithology map over the bounds that name a rock: [(geometry in EPSG:3857, code)]
    per map, EGDI first."""
    import pyogrio
    import shapely
    from pyproj import Transformer

    out = []
    for path, (layer, field, terms) in zip(paths, layers):
        codes = {term: vocabulary.lithology[name] for name, words in terms.items() for term in words.split("|")}
        crs = pyogrio.read_info(path, layer=layer)["crs"]
        to_map = Transformer.from_crs("EPSG:4326", crs, always_xy=True)
        to_tiles = Transformer.from_crs(crs, "EPSG:3857", always_xy=True)
        meta, _, wkb, (values,) = pyogrio.raw.read(path, layer=layer, columns=[field], bbox=to_map.transform_bounds(*bounds, densify_pts=21))
        units = []
        for geometry, value in zip(shapely.from_wkb(wkb), values):
            # EGDI names a rock by a URI, IGME 5000 by terms such as "dolomite/dolostone".
            value = value or ""
            code = codes.get(value) or codes.get(value.rsplit("/", 1)[-1])
            if code and geometry is not None:
                units.append((shapely.transform(geometry, lambda xy: np.stack(to_tiles.transform(xy[:, 0], xy[:, 1]), axis=-1)), code))
        out.append(units)
    return out


class Lithology:
    """The rock of each texel from EGDI, and from IGME 5000 where EGDI names none."""

    def __init__(self, maps):
        self.maps = [Polygons(units) for units in maps]

    def __call__(self, z, x, y):
        transform = tiles.transform(z, x, y)
        out = np.zeros((tiles.TILE, tiles.TILE), np.uint8)
        for units in self.maps:
            unknown = out == 0
            out[unknown] = units.burn(transform)[unknown]
        return out


def area_of(where):
    if not isinstance(where[0], list):
        w, s, e, n = where
        return (e - w) * (n - s)
    return abs(sum(x0 * y1 - x1 * y0 for (x0, y0), (x1, y1) in zip(where, where[1:] + where[:1]))) / 2


def ring(where):
    if isinstance(where[0], list):
        return where
    w, s, e, n = where
    return [[w, s], [e, s], [e, n], [w, n]]


def shape(rings):
    """A GeoJSON polygon of [lon, lat] rings in EPSG:3857 metres."""
    return {"type": "Polygon", "coordinates": [np.stack(mercator_m(*np.array(r, float).T), axis=-1).tolist() for r in rings]}


class Region:
    """The region of each texel: the ecoregion that holds it, or the nearest within REACH_M, else
    `world`; from there it steps into the subregion whose `where` holds it, the smallest on a tie,
    until none does. Ids are indexes into the regions of `regions.json`."""

    def __init__(self, regions, bounds):
        from rasterio.features import rasterize

        self.ids = {region["id"]: i for i, region in enumerate(regions["regions"])}
        self.subregions = {}
        for region in regions["regions"]:
            for where in region.get("where", []):
                self.subregions.setdefault(region.get("extends"), []).append((area_of(where), region["id"], where))
        self.ecoregions = [(self.ids[e["short"]], polygon) for e in regions["ecoregions"] for polygon in e["polygons"]]
        # The fallback over the bounds and REACH_M around them, on whole tiles of FALLBACK_ZOOM.
        lat = max(abs(bounds[1]), abs(bounds[3]))
        pad = math.degrees(REACH_M / 6371008.8)
        wide = [bounds[0] - pad / math.cos(math.radians(lat)), bounds[1] - pad, bounds[2] + pad / math.cos(math.radians(lat)), bounds[3] + pad]
        cover = tiles.covered(wide, FALLBACK_ZOOM)
        (x0, y0), (x1, y1) = min(cover), max(cover)
        self.origin = (x0 * tiles.TILE, y0 * tiles.TILE)
        size = ((y1 - y0 + 1) * tiles.TILE, (x1 - x0 + 1) * tiles.TILE)
        index = self.ecoregion(rasterize, tiles.transform(FALLBACK_ZOOM, x0, y0), size)
        steps = round(REACH_M / tiles.texel_m(FALLBACK_ZOOM, (y0 + y1) // 2))
        self.fallback = spread(index, index == REGION_NO_DATA, self.ids["world"], steps=steps)

    def ecoregion(self, rasterize, transform, size):
        """The id of the ecoregion's region over each texel, REGION_NO_DATA under none."""
        shapes = [(shape(polygon), code) for code, polygon in self.ecoregions]
        return (rasterize(shapes, out_shape=size, transform=transform, fill=REGION_NO_DATA, dtype=np.uint8) if shapes
                else np.full(size, REGION_NO_DATA, np.uint8))

    def __call__(self, z, x, y):
        from rasterio.features import rasterize

        transform = tiles.transform(z, x, y)
        index = self.ecoregion(rasterize, transform, (tiles.TILE, tiles.TILE))
        gaps = index == REGION_NO_DATA
        if gaps.any():
            scale = 1 << (z - FALLBACK_ZOOM)
            rows = (y * tiles.TILE + np.arange(tiles.TILE)) // scale - self.origin[1]
            cols = (x * tiles.TILE + np.arange(tiles.TILE)) // scale - self.origin[0]
            index[gaps] = self.fallback[rows[:, None], cols[None, :]][gaps]
        names = {i: name for name, i in self.ids.items()}
        queue = [int(code) for code in np.unique(index)]
        while queue:
            code = queue.pop(0)
            held = index == code
            # The smallest burnt last, so it wins a tie.
            for _, name, where in sorted(self.subregions.get(names[code], []), reverse=True):
                inside = rasterize([(shape([ring(where)]), 1)], out_shape=index.shape, transform=transform, dtype=np.uint8).astype(bool) & held
                if inside.any():
                    index[inside] = self.ids[name]
                    if self.ids[name] not in queue:
                        queue.append(self.ids[name])
        return index


def step():
    request = step_request.read()
    options, layers = request["options"], request["layers"]
    bounds, layer = options["bounds"], options["layer"]
    rules = layers["planner/trailview-rules"]
    metadata = {"trailview_format": tiles.FORMAT, "layer": layer, "attribution": options["attribution"]}
    if layer == "region":
        bake, top = Region(json.loads(Path(rules["regions.json"]).read_bytes()), bounds), REGION_ZOOM
    else:
        vocabulary = Vocabulary(json.loads(Path(rules["materials.json"]).read_bytes()))
        if layer == "lithology":
            files = [step_request.files(request, source) for source in ("egdi-geology", "igme5000")]
            paths = [next(path for name, path in files[0].items() if name.endswith(".gpkg")), igme_shapefile(files[1])]
            bake, top = Lithology(lithology_units(paths, (EGDI, IGME5000), bounds, vocabulary)), LITHOLOGY_ZOOM
        else:
            from . import colour

            forests, areas = osm_areas(layers["planner/osm"]["osm.pbf"], vocabulary)
            mosaics = colour.mosaics(step_request.files(request, "sentinel-2-mosaics"))
            corine = clc_images(step_request.files(request, "corine-land-cover"), vocabulary)
            worldcovers = sorted(step_request.files(request, "esa-worldcover").values())
            bake, top = Material(vocabulary, bounds, worldcovers, corine, forests, areas, mosaics), MATERIAL_ZOOM
    sizes = bake_layer(layer, bake, top, bounds, Path(request["output"]), metadata)
    step_request.metrics(request, {"bytes_per_zoom": sizes, "bytes_per_km2": round(sum(sizes.values()) / tiles.km2(bounds), 1)})


def bake_layer(layer, bake, top, bounds, output, metadata):
    """Write `<layer>.pmtiles` from zoom 0 to `top`; return its bytes per zoom. A tile with no data is
    left out."""
    no_data = {"material": NO_DATA, "region": REGION_NO_DATA}.get(layer)
    encode = lambda texels: {} if no_data is not None and (texels == no_data).all() else {layer: tiles.lossless(texels)}
    sizes = tiles.pyramid(bounds, range(top + 1), bake, lambda children: tiles.mode(children, no_data), encode, output, metadata)
    return sizes.get(layer, {})


def igme_shapefile(files):
    """The GDAL path of the polygon shapefile in the IGME 5000 archive, read in place. The braces let
    GDAL open an archive whose name, as a store object's, has no `.zip`."""
    import zipfile

    archive, = files.values()
    with zipfile.ZipFile(archive) as z:
        member = next(name for name in z.namelist() if name.endswith(f"/{IGME5000[0]}.shp"))
    return f"/vsizip/{{{archive}}}/{member}"


def fetch_corine(bounds, version, out):
    """The capture of `corine-land-cover`: the legend, and the map images that cover the bounds in
    EPSG:3035 at CLC_PX_M, CLC_MAX_PX at most per side, each named by its box in whole metres."""
    from pyproj import Transformer

    from .. import planner_runtime as runtime

    service = CLC_SERVICE.format(version=version)
    out.mkdir(parents=True, exist_ok=True)
    with runtime.open_url(f"{service}/legend?f=json") as response:
        (out / "legend.json").write_bytes(response.read())
    west, south, east, north = Transformer.from_crs("EPSG:4326", "EPSG:3035", always_xy=True).transform_bounds(*bounds, densify_pts=21)
    snap = lambda v, rounding: int(rounding(v / CLC_PX_M) * CLC_PX_M)
    west, south, east, north = snap(west, math.floor), snap(south, math.floor), snap(east, math.ceil), snap(north, math.ceil)
    step_m = int(CLC_PX_M * CLC_MAX_PX)
    for x in range(west, east, step_m):
        for y in range(south, north, step_m):
            box = (x, y, min(x + step_m, east), min(y + step_m, north))
            target = out / f"clc-{box[0]}_{box[1]}_{box[2]}_{box[3]}.png"
            if target.exists():
                continue
            params = {"bbox": ",".join(map(str, box)), "bboxSR": 3035, "imageSR": 3035, "layers": "show:1", "format": "png32",
                      "size": f"{round((box[2] - box[0]) / CLC_PX_M)},{round((box[3] - box[1]) / CLC_PX_M)}",
                      "transparent": "true", "f": "image"}
            with runtime.open_url(f"{service}/export?{urllib.parse.urlencode(params)}", timeout=300) as response:
                part = target.with_name(target.name + ".part")
                part.write_bytes(response.read())
                part.replace(target)
            print(f"corine {target.name}", file=sys.stderr, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bounds", type=geo.bounds, required=True)
    parser.add_argument("--version", default="2018")
    parser.add_argument("--fetch-corine", type=Path, required=True, help="write the capture of corine-land-cover here")
    args = parser.parse_args()
    fetch_corine(args.bounds, args.version, args.fetch_corine)


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
