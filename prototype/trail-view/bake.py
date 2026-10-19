# /// script
# requires-python = ">=3.12"
# dependencies = ["rasterio==1.5.2", "pyproj==3.8.0", "numpy==2.5.3", "truststore==0.10.4", "timezonefinder==9.0.0"]
# ///
"""Bake one Trail View spot into ~/.cache/obc/trail-view/<spot>/.

    uv run prototype/trail-view/bake.py hahnen

Every raster is warped into one azimuthal-equidistant frame centred on the eye, so x and y are
metres east and north. Level k covers ±1.25 km · 2^k: HEIGHT_SIZE² heights, SIZE² texels
for every other layer.
"""

import argparse
import base64
import fcntl
import gzip
import http.client
import json
import math
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import warnings
import zipfile
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from pathlib import Path

import numpy as np
import rasterio
import truststore
from pyproj import CRS, Transformer
from timezonefinder import TimezoneFinder

# Certificates are checked against the system's trust store, as curl does: Spain's IGN serves a
# chain whose root Python's own bundle does not hold.
truststore.inject_into_ssl()
# Map images come without a georeference; their box is the request's.
warnings.filterwarnings("ignore", category=rasterio.errors.NotGeoreferencedWarning)
# urllib tries every IPv6 address for its full timeout before IPv4, and IPv6 to some CDNs hangs.
_getaddrinfo = socket.getaddrinfo
socket.getaddrinfo = lambda host, port, family=0, *rest, **kw: _getaddrinfo(host, port, socket.AF_INET, *rest, **kw)

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path[:0] = [str(REPO / "host/obc-dem/reference"), str(REPO)]
from ingest.sources import SOURCES as DTM_SOURCES  # noqa: E402
from ingest.sources.base import http_download  # noqa: E402
from ingest.sources.es import ES  # noqa: E402
from ingest.sources.fr import FR, BilWmsSource  # noqa: E402
from ingest.sources.protocols import ArcGisSource  # noqa: E402
from ingest.sources.stac import stac_items  # noqa: E402
from ingest.sources.us import US  # noqa: E402
from tools import data_registry, planner_snow, planner_sources  # noqa: E402
from tools.planner_snow import cdse_credentials  # noqa: E402

CACHE = Path.home() / ".cache/obc/trail-view"
SOURCES = CACHE / "_sources"

LEVELS = 9
SIZE = 1024
# Heights lie on a finer grid than the other layers, so the k = 3 level still holds the national
# grid's 10 m: at SIZE its texels are 20 m.
HEIGHT_SIZE = 2048
HALF0_M = 1250.0
# The national DTM covers k ≤ 3, ±10 km. The owner chose it after ±5 km put the Rigidalstock
# ridge, 4.7 km from Brunni, into the blend toward GLO-30.
DTM_LEVELS = 4
# The owner chose a 10 m grid, cubic from the finest national DTM, as the sweet spot of detail
# against the data a Europe-wide grid needs. Every DTM level is read from it.
NATIONAL_GRID_M = 10.0
# The height above the ground the Peak View photographs were taken from. A spot can raise its
# eye with `eye_above_ground_m`, as a lookout's deck stands above the summit rocks.
PHONE_M = 1.6
NODATA = -32768.0

GLO30_LOCAL = Path.home() / ".cache/obcm/dem"
GLO30_S3 = "https://copernicus-dem-30m.s3.amazonaws.com"
GLO30_PX_M = 30.0
PHOTO_JSON = REPO / "fixtures/sources/peak-view/photos"
# 3DEP at 2 m: the registry's 1 m row fetches four times the pixels the levels can show.
US_2M = ArcGisSource("us", "United States", "3DEP 2 m", 2.0, "Public domain", "NAVD88", url=US.service)
# RGE ALTI at 2 m for the same reason: at 1 m a ±10 km window is 400 M pixels.
FR_2M = BilWmsSource("fr", "France", "RGE ALTI 2 m", 2.0, FR.licence, FR.vertical_datum, url=FR.service, layer=FR.layer)
# Services that answer one box at a time. request_boxes sizes a split at the box's middle
# latitude, so a row further from the pole can overrun a service's 2000 px cap; 3 km boxes stay
# far below it at these resolutions.
BOXED = {"us": US_2M, "fr": FR_2M, "es": ES}
# Scotland's Phase 1 LiDAR DTM: 1 m GeoTIFFs in British National Grid, one per 10 km OS square,
# on a public bucket. It was flown in patches (Glen Coe, not Torridon or Skye).
SCOT_LIDAR = "https://srsp-open-data.s3.eu-west-2.amazonaws.com/lidar/phase-1/dtm/27700/gridded/{}_1M_DTM_PHASE1.tif"
BOX_M = 3000
# Public Overpass instances, tried in turn while one is busy.
OVERPASS = ("https://overpass-api.de/api/interpreter", "https://overpass.kumi.systems/api/interpreter")
# A local Overpass where one is installed: bin/osm3s_query and db/. It holds only the tags these
# queries ask for (README, "Local Overpass").
LOCAL_OVERPASS = SOURCES / "overpass"
USER_AGENT = "OpenBikeComputer trail view prototype (https://github.com/timohueser/OpenBikeComputer)"

WORLDCOVER = "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_{}_Map.tif"
WORLDCOVER_PX_M = 10.0
# The ground vocabulary the page shares: a material's or lithology's code is its index.
VOCABULARY = json.loads((HERE / "materials.json").read_text())
MATERIAL = {entry["id"]: code for code, entry in enumerate(VOCABULARY["materials"])}
LITHOLOGY = {entry["id"]: code for code, entry in enumerate(VOCABULARY["lithologies"])}
COVER = np.array([entry["cover"] for entry in VOCABULARY["materials"]])
WOODLAND = COVER == "tree"
NONE, WATER, SEA = MATERIAL["none"], MATERIAL["inland-water"], MATERIAL["sea"]
WORLDCOVER_MATERIALS = {10: "forest", 20: "scrub", 30: "grassland", 40: "arable", 50: "built-up", 60: "sparse",
                        70: "glacier", 80: "inland-water", 90: "marsh", 95: "marsh", 100: "moss-lichen"}
# The materials a coarser map may name more precisely: the same structure, or open ground of
# another name. WorldCover's 10 m keeps the structure; a 100 m map only names it. The satellite
# finds heath on open ground (MOOR_ON) and tells it from green grass and scrub, which a map's heath
# or bog then leaves as they are.
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
# Heath from the satellite: open ground that reflects as much red as green (the midpoint of 1.1
# to 1.4) while it grows. Mediterranean summer ground is as red, but dry: its NDVI is low and past
# its spring peak.
MOOR_GREEN_RED = 1.25
MOOR_NDVI = 0.45
MOOR_PEAK = 0.1
# Not on sparse ground: a green pixel there is grass beside bare stone, as on a river's gravel bar.
MOOR_ON = OPEN - {"sparse"}
# Land cover and geology maps refine the baseline on k < NATIONAL_LEVELS (±5 km).
NATIONAL_LEVELS = 3

S2_STAC = "https://stac.dataspace.copernicus.eu/v1/collections/sentinel-2-global-mosaics/items"
S2_YEAR = 2025
S2_BANDS = ("B02", "B03", "B04", "B08", "observations")
# Colour reaches k ≤ 6, ±80 km; beyond it, haze dominates and the class palette is enough.
S2_LEVELS = 7
# A pixel with fewer valid looks in its quarter falls back to the palette.
MIN_OBSERVATIONS = 3
# Below this visible reflectance a winter pixel is mostly terrain shadow, so its chroma comes
# from summer.
DARK = 0.025
# The alpha code of a quarter's texel that saw snow (snow_free); a measured NDVI never takes it.
SNOW_MARK = 1

# The planner's snow layer (specs/planner-snow-tiles.md) on the levels: per texel and past season,
# the onset and melt-out day step of the longest snow period, from HR-WSI where it has data and
# MODIS elsewhere, as tools/planner_snow.py reads them, over the planner's seasons.
FIRST_SEASON, LAST_SEASON = tomllib.loads((REPO / "data/planner.toml").read_text())["snow"]["seasons"]
SNOW_SEASONS = range(FIRST_SEASON, LAST_SEASON + 1)
# Level 3's 19.5 m texels hold HR-WSI's 20 m; the finer levels read level 3.
SNOW_FIRST = 3
# HR-WSI is warped nearest onto a grid this many times finer than the texels, from the overview
# nearest it, and each block blends into its texel by the planner's blend rule.
SNOW_OVERSAMPLE = 4
SNOW_STRIP = 128
# MODIS is read once for the box of the last level, snapped out to this, so nearby spots share it.
MODIS_SNAP_DEG = 0.25

# The near field: road surfaces on k < ROAD_LEVELS (±5 km), buildings within BUILDING_HALF_M,
# instanced trees within TREE_RADIUS_M on a jittered grid of TREE_SPACING_M, about 1.5 trees per
# 10 m WorldCover pixel.
ROAD_LEVELS = 3
BUILDING_HALF_M = 2500.0
TREE_RADIUS_M = 1000.0
TREE_SPACING_M = 8.0
# Road surfaces in the order the page's shader paints them (code = index + 1, 0 = no road), with
# the half-width each is painted at where OSM gives no width. The field holds the distance to the
# nearest road EDGE, which stays continuous where two roads of different widths meet; a byte is
# 0.1 m from -5 m to 20.5 m.
ROAD_WIDTHS = {"main": 4.5, "road": 3.0, "gravel": 1.75, "dirt": 1.5, "grass": 1.25, "path": 0.6}
# Paved half-widths by class where OSM gives no width: a lane is about one car wide.
PAVED_WIDTHS = {"motorway": 6.0, "trunk": 5.0, "primary": 4.0, "secondary": 3.25, "tertiary": 3.0,
                "unclassified": 2.5, "residential": 2.5, "service": 2.0, "living_street": 2.0}
ROAD_REACH_M = 20.5
# Roads are graded: level across, smoothed along over ±GRADE_SMOOTH_M, and the ground beside them
# cut or filled to them at no more than GRADE_BANK (rise over run, about 40°), as a road is built.
GRADE_SMOOTH_M = 12.0
GRADE_BANK = 0.85
# OSM's leaf_type, and a region's `leaf`, as the forest it makes.
LEAF = {"needleleaved": "needleleaf-forest", "broadleaved": "broadleaf-forest", "mixed": "mixed-forest"}
# Tree heights where no DTM gives a canopy: hedgerow and garden trees.
NO_CANOPY_M = (5.0, 9.0)
# OSM water on k < WATER_LEVELS (±10 km), as coverage from a 4× finer grid: rivers and canals
# narrower than a WorldCover pixel, and lakes. Full widths for a waterway without a width tag.
# Beyond, WorldCover's water class carries what is wide enough to see.
WATER_LEVELS = 4
# Summits come from Overpass in tiles of SUMMIT_TILE_DEG that all spots share: the last level
# spans ±320 km, so neighbouring spots need nearly the same summits.
SUMMIT_TILE_DEG = 5
WATERWAY_WIDTHS = {"river": 12.0, "canal": 8.0, "stream": 2.0}
# WorldCover water that no OSM water holds stays water only where its surface is this flat across
# 3×3 texels, as a lake's is: dark gullies and cliff shadows read as water too.
FLAT_WATER_M = 1.0
# With a coastline, WorldCover water within this many metres of the sea and outside it is beach,
# rock or surf it misread, unless OSM maps water there.
COAST_SHORE_M = 50.0
SUPERSAMPLE = 4
# The water levels' second byte: metres from water to the nearest land as sqrt(d / SHORE_REACH_M),
# so the shallows near a shore keep their detail and open water reaches the deep colour.
SHORE_REACH_M = 600.0
# Water at most this high is the sea, for cliff coasts.
SEA_LEVEL_M = 3.0
# A cliff rim's lift is averaged over this radius of land.
RIM_SMOOTH_M = 5.0
# OSM bridge values for a way that lies on the ground: a road like any other, not a deck.
GROUND_BRIDGES = {"no", "boardwalk", "low_water_crossing"}
# A spot names its data sources (dtm, landcover, geology); its region, the ruleset in regions/
# that region_of finds, says how its terrain is shaped. spots/<region>.json holds the spots whose
# region walk starts at regions/<region>.json.
SPOTS = HERE / "spots"
REGIONS = HERE / "regions"
# Countries and their first-level subdivisions (tune/countries.py) and the ecoregions
# (tune/ecoregions.py) are simplified to about a kilometre: a point outside every one takes the
# nearest within this reach, as a cliff top the simplified coast leaves in the sea.
COUNTRIES = HERE / "tune/countries.json"
ECOREGIONS = HERE / "tune/ecoregions.json"
REACH_M = 25_000.0
# Where two regions meet, their terrain rules blend over this width, so no seam shows.
FEATHER_M = 200.0

# CORINE Land Cover 2018 at 100 m over Europe: the EEA's map service draws it in its legend's
# colours, which name its classes. The classes that make a baseline material more precise.
CLC = ("https://image.discomap.eea.europa.eu/arcgis/rest/services/Corine/CLC2018_LAEA/MapServer",
       "Corine Land Cover 2018 © European Union, Copernicus Land Monitoring Service")
CLC_PX_M = 100.0
CLC_MATERIALS = {131: "bare-soil", 132: "bare-soil", 213: "rice", 221: "vineyard", 222: "orchard", 223: "olive-grove",
                 231: "pasture", 311: "broadleaf-forest", 312: "needleleaf-forest", 313: "mixed-forest", 321: "rough-grass",
                 322: "heath", 323: "sclerophyllous-scrub", 331: "sand", 332: "bare-rock", 412: "bog", 421: "salt-marsh"}
# OSM's natural and landuse areas, and its kinds of wetland, that name a material more precisely.
OSM_NATURAL = {"bare_rock": "bare-rock", "scree": "scree", "shingle": "beach", "sand": "sand", "beach": "beach",
               "heath": "heath", "grassland": "rough-grass"}
OSM_LANDUSE = {"vineyard": "vineyard", "orchard": "orchard"}
WETLANDS = {"bog": "bog", "saltmarsh": "salt-marsh", "wet_meadow": "wet-grassland"}

# The rock of every land texel: EGDI's 1:1 000 000 surface geology of Europe, and BGR's 1:5 000 000
# IGME 5000 where EGDI names no rock (Ireland; Britain gives only its surface deposits). Each is one
# download: the URL, its layer, the field that names the rock, and the credit.
EGDI = ("https://egeologija.si/download/GeologicUnitView.gpkg", "GeologicUnitView", "representativelithology_uri",
        "Surface geology 1:1 000 000 © EuroGeoSurveys (EGDI), CC BY 4.0")
IGME5000 = ("https://download.bgr.de/bgr/Geologie/IGME5000/shp/IGME5000.zip", "IGME5000_europeEPSG3034shp_geology_poly_v01",
            "Portr_Petr", "IGME5000, BGR")
# Their terms for each lithology, split by "|". Any other term is unknown: surface deposits, which
# hide the rock that crags show, and undifferentiated units.
EGDI_TERMS = {
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
}
IGME5000_TERMS = {
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
}

# Spain's continuous 1:50 000 geological map (IGME, not IGME 5000) on k < NATIONAL_LEVELS: layer 1
# is the units' fill. Its colours are cartographic, so each unit's own description names its rock.
GEOLOGY = {"igme": ("https://mapas.igme.es/gis/services/Cartografia_Geologica/IGME_Geode_50/MapServer/WMSServer", "1",
                  "GEODE 1:50 000 © IGME")}
# Words in a unit's description for each lithology. A description names its main rock first, so
# the earliest word wins, and names variegated beds right after their rock ("arcillas abigarradas").
# Descriptions write "basálticas" with the accent.
ROCK_WORDS = {
    "limestone-marl": ("alternancia", "flysch", "margos"), "gypsum": ("yeso",), "limestone": ("calcaren", "caliza"),
    "dolomite": ("dolom",), "sandstone": ("arenisca",), "conglomerate": ("conglomerad",), "marl-clay": ("marga", "arcilla", "limo"),
    "schist-slate-shale": ("pizarra", "esquisto"), "granite": ("granit",), "quartzite": ("cuarcita",),
    "alluvium-colluvium": ("glacis", "coluvi", "aluvi", "terraza", "deyecc", "deslizamiento", "arroyada"),
    "basalt-lava": ("colada", "basalt", "basált", "ankaramit"), "felsic-lava": ("fonolit", "fonolít", "traqui", "sálic"),
    "dolerite": ("dique", "intrusiv"), "cinder-scoria": ("escoria", "piroclast"), "pumice": ("pumit", "pómez"), "tuff": ("toba",),
}
VARIEGATED, VARIEGATED_AFTER = "abigarrad", 3
# Ireland's National Land Cover Map 2018, mapped from 1 m imagery to 0.05 ha: the feature service,
# and its LEVEL_2_VALUE words for each material, the first match winning. Its buildings, ways and
# artificial surfaces name no material: OSM draws the houses and roads, and the polygons hold their
# verges and yards.
NLCM = ("https://services-eu1.arcgis.com/FH5XCsx8rYXqnjF5/arcgis/rest/services/MapGenieNationalLandCover2018ITM/FeatureServer/0",
        "National Land Cover Map 2018 © Tailte Éireann and EPA, CC BY 4.0")
NLCM_WORDS = [("exposed rock", "bare-rock"), ("heath", "heath"), ("bog", "bog"), ("peat", "bog"), ("fen", "marsh"),
              ("swamp", "marsh"), ("bracken", "bracken"), ("wet grassland", "wet-grassland"), ("dry grassland", "rough-grass"),
              ("improved grassland", "pasture"), ("amenity grassland", "pasture"), ("scrub", "scrub"),
              ("transitional forest", "scrub"), ("hedgerow", "hedge"), ("treeline", "hedge"), ("sand", "sand"),
              ("coastal sediments", "beach"), ("mudflat", "sand"), ("saltmarsh", "salt-marsh"), ("bare soil", "bare-soil")]
# NatureScot's Habitat Map of Scotland, without overlaps or mosaics, over WFS; its EUNIS codes for
# each material, the longest matching prefix winning. Woodland keeps WorldCover's trees; constructed
# ground (J) names no material, as NLCM's does not.
HABMOS = ("https://ogc.nature.scot/geoserver/ows", "habitatsandspecies:habmosnvccleaned",
          "Contains NatureScot information licensed under the Open Government Licence v3.0")
HABMOS_CODES = {"F4": "heath", "F2": "heath", "D1": "bog", "D2": "bog", "D4": "marsh", "H2": "scree", "H3": "bare-rock",
                "B3": "bare-rock", "E5.3": "bracken", "E5": "rough-grass", "E1": "rough-grass", "E3": "wet-grassland",
                "E4": "rough-grass", "E2": "pasture", "I": "pasture", "F3": "scrub", "F9": "scrub", "G5": "scrub", "B1": "sand",
                "A2.5": "salt-marsh"}

# OSM features near the eye: walls and hedges within WALL_HALF_M, cut into pieces of at most
# WALL_PIECE_M so each stands on the ground; orchard trees in rows ORCHARD_SPACING_M apart.
WALL_HALF_M = 1500.0
WALL_PIECE_M = 8.0
ORCHARD_SPACING_M = 7.0

# Many Linux GDAL packages install it as gdal_proximity.py only.
PROXIMITY = shutil.which("gdal_proximity") or "gdal_proximity.py"
GDAL_OPTIONS = {"GDAL_DISABLE_READDIR_ON_OPEN": "EMPTY_DIR", "CPL_VSIL_CURL_ALLOWED_EXTENSIONS": ".tif,.vrt",
                "GDAL_HTTP_MAX_RETRY": "4", "GDAL_HTTP_RETRY_DELAY": "5"}

CREDITS = {
    "glo30": "Copernicus DEM GLO-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018, provided under COPERNICUS by the European Union and ESA",
    "ch": "swissALTI3D © swisstopo",
    "de-bw": "DGM1 © LGL Baden-Württemberg, dl-de/by-2-0",
    "us": "USGS 3D Elevation Program, public domain",
    "fr": "RGE ALTI © IGN, Licence Ouverte",
    "es": "MDT05 © Instituto Geográfico Nacional, CC BY 4.0",
    "scot": "LiDAR Phase 1 DTM, Crown copyright Scottish Government, SEPA and Scottish Water (2012), OGL v3",
    "worldcover": "ESA WorldCover 10 m 2021 v200 © ESA WorldCover project, CC BY 4.0",
    "s2": f"Contains modified Copernicus Sentinel data {S2_YEAR} (Sentinel-2 quarterly mosaics)",
    "osm": "© OpenStreetMap contributors (ODbL)",
    "photo": "Photograph © Timo Hüser, CC BY 4.0",
}


@contextmanager
def machine_lock(name: str):
    """`name` for this bake alone among the bakes on this machine; the kernel frees it when one dies."""
    path = SOURCES / "locks" / f"{name}.lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print(f"  waiting for {name}", flush=True)
            fcntl.flock(handle, fcntl.LOCK_EX)
        yield


def write_atomic(path: Path, text: str) -> None:
    """The whole file or none of it, so a bake cut short leaves no half file for the next to read."""
    part = path.with_name(f"{path.name}.{os.getpid()}.part")
    part.parent.mkdir(parents=True, exist_ok=True)
    part.write_text(text, encoding="utf-8")
    os.replace(part, path)


def fixture_photo(name: str) -> Path:
    """The photograph of the `peak-view-photos` fixture package, as `obc_fixtures` finds it."""
    if root := os.environ.get("OBC_FIXTURE_CACHE"):
        base = Path(root) / "by-id"
    elif root := os.environ.get("XDG_CACHE_HOME"):
        base = Path(root) / "openbikecomputer/fixtures/by-id"
    else:
        base = Path.home() / ".cache/openbikecomputer/fixtures/by-id"
    return base / "peak-view-photos/photos" / f"{name}.jpg"


def merge(a: dict, b: dict) -> dict:
    """b over a: objects merge, anything else replaces."""
    out = dict(a)
    for key, value in b.items():
        out[key] = merge(a[key], value) if isinstance(value, dict) and isinstance(a.get(key), dict) else value
    return out


def holds(area: list, lat: float, lon: float) -> bool:
    """Whether a box [west, south, east, north] or a ring of [lon, lat] holds the point."""
    if not isinstance(area[0], list):
        w, s, e, n = area
        return w <= lon <= e and s <= lat <= n
    inside = False
    for (x0, y0), (x1, y1) in zip(area, area[1:] + area[:1]):
        if (y0 > lat) != (y1 > lat) and lon < x0 + (lat - y0) * (x1 - x0) / (y1 - y0):
            inside = not inside
    return inside


def area_of(area: list) -> float:
    if not isinstance(area[0], list):
        w, s, e, n = area
        return (e - w) * (n - s)
    return abs(sum(x0 * y1 - x1 * y0 for (x0, y0), (x1, y1) in zip(area, area[1:] + area[:1]))) / 2


def region_table() -> dict[str, dict]:
    return {path.stem: json.loads(path.read_text()) for path in REGIONS.glob("*.json")}


def rules_of(table: dict, name: str) -> dict:
    """The region's rules, its parents' merged under them."""
    chain = [name]
    while "extends" in table[chain[0]]:
        chain.insert(0, table[chain[0]]["extends"])
    region = {}
    for link in chain:
        region = merge(region, {k: v for k, v in table[link].items() if k not in ("extends", "where", "about")})
    return region


def region_of(table: dict, start: str, lat: float, lon: float) -> list[str]:
    """The region ids from world down to the point's region. The walk starts at `start` and steps
    into the subregion of the current region whose `where` holds the point, the smallest on a tie,
    until none does."""
    if start not in table:
        sys.exit(f"spots/{start}.json has no region: regions/{start}.json is missing")
    chain = [start]
    while "extends" in table[chain[0]]:
        chain.insert(0, table[chain[0]]["extends"])
    while fits := [(area_of(area), name) for name, region in table.items() if region.get("extends") == chain[-1]
                   for area in region.get("where", []) if holds(area, lat, lon)]:
        chain.append(min(fits)[1])
    return chain


def frame_ring(ring: list, to_frame, step_deg: float = 0.02) -> list:
    """A ring of [lon, lat] in frame metres, with vertices every step_deg along each edge, so its
    edges curve as they do in the frame."""
    out = []
    for (x0, y0), (x1, y1) in zip(ring, ring[1:] + ring[:1]):
        n = max(1, math.ceil(max(abs(x1 - x0), abs(y1 - y0)) / step_deg))
        out += [(x0 + (x1 - x0) * i / n, y0 + (y1 - y0) * i / n) for i in range(n)]
    xs, ys = to_frame.transform(*np.array(out).T)
    return np.stack([xs, ys], axis=-1).tolist() + [[float(xs[0]), float(ys[0])]]


def region_levels(table: dict, start: str, to_frame, to_wgs84) -> tuple[list[np.ndarray], list[str]]:
    """Per level, R8: the index into the names, the regions present, of the region that each texel
    lies in. A texel lies in the ecoregion that holds it, or the nearest within REACH_M, else in
    `start`; from there it steps into subregions as region_of's walk does. Coarse levels first: a
    texel no ecoregion holds takes the region of the coarser level under it."""
    from rasterio.features import rasterize

    names = [start]

    def code(name: str) -> int:
        if name not in names:
            names.append(name)
        return names.index(name)

    ecoregions = [e for e in json.loads(ECOREGIONS.read_text())["regions"] if e.get("short") in table]
    levels = [None] * LEVELS
    for k in reversed(range(LEVELS)):
        half = HALF0_M * 2**k
        transform = rasterio.transform.from_bounds(-half, -half, half, half, SIZE, SIZE)
        west, south, east, north = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
        shapes = [({"type": "Polygon", "coordinates": [frame_ring(ring, to_frame) for ring in polygon]}, code(e["short"]))
                  for e in ecoregions for polygon in e["polygons"]
                  if min(x for x, _ in polygon[0]) < east and max(x for x, _ in polygon[0]) > west
                  and min(y for _, y in polygon[0]) < north and max(y for _, y in polygon[0]) > south]
        index = rasterize(shapes, out_shape=(SIZE, SIZE), transform=transform, fill=255, dtype=np.uint8) \
            if shapes else np.full((SIZE, SIZE), 255, np.uint8)
        gaps = index == 255
        if k == LEVELS - 1:
            index = spread(index, gaps, 0, steps=round(REACH_M / (2 * half / SIZE)))
        else:
            under = levels[k + 1][SIZE // 4:3 * SIZE // 4, SIZE // 4:3 * SIZE // 4].repeat(2, axis=0).repeat(2, axis=1)
            index[gaps] = under[gaps]
        # Each region in turn steps into its subregions, the smallest burnt last so it wins a tie.
        n = 0
        while n < len(names):
            inside = index == n
            subregions = [(area_of(area), name, area) for name, region in table.items()
                          if region.get("extends") == names[n] for area in region.get("where", [])] if inside.any() else []
            for _, name, area in sorted(subregions, reverse=True):
                ring = area if isinstance(area[0], list) else [[area[0], area[1]], [area[2], area[1]], [area[2], area[3]],
                                                                [area[0], area[3]]]
                held = rasterize([({"type": "Polygon", "coordinates": [frame_ring(ring, to_frame)]}, 1)], out_shape=(SIZE, SIZE),
                                 transform=transform, dtype=np.uint8).astype(bool) & inside
                if held.any():
                    index[held] = code(name)
            n += 1
        levels[k] = index
    present = sorted(set().union(*(np.unique(index).tolist() for index in levels)))
    remap = np.zeros(256, np.uint8)
    remap[present] = np.arange(len(present))
    return [remap[index] for index in levels], [names[n] for n in present]


def by_region(heights: np.ndarray, regions: np.ndarray, rules: list[dict], half: float, shape) -> np.ndarray:
    """shape(heights, rules) for each region's rules, blended where the regions lie: each one's
    share is its area feathered over FEATHER_M."""
    present = np.unique(regions)
    if len(present) == 1:
        return shape(heights, rules[present[0]])
    r = max(1, round(FEATHER_M / (2 * half / SIZE)))
    return sum(fine(box_mean((regions == code).astype(np.float32), r)) * shape(heights, rules[code]) for code in present)


def place_at(places: list[dict], lat: float, lon: float) -> str | None:
    """The lower-case ISO code of the place that holds the point, else of the nearest one within
    REACH_M."""
    nearest, reach = None, REACH_M
    for place in places:
        (x0, y0), (x1, y1) = np.array([pair for ring in place["rings"] for pair in zip(ring, ring[1:])]).transpose(1, 2, 0)
        # Even-odd over every ring of the place: a hole's crossings cancel its polygon's.
        with np.errstate(divide="ignore", invalid="ignore"):
            crossings = ((y0 > lat) != (y1 > lat)) & (lon < x0 + (lat - y0) * (x1 - x0) / (y1 - y0))
        if crossings.sum() % 2:
            return place["iso"]
        # Metres on a plane through the point: close enough to tell REACH_M.
        sx, sy = 111_320.0 * math.cos(math.radians(lat)), 111_320.0
        ax, ay, dx, dy = (x0 - lon) * sx, (y0 - lat) * sy, (x1 - x0) * sx, (y1 - y0) * sy
        t = np.clip(-(ax * dx + ay * dy) / np.maximum(dx * dx + dy * dy, 1e-9), 0, 1)
        if (d := float(np.hypot(ax + t * dx, ay + t * dy).min())) < reach:
            nearest, reach = place["iso"], d
    return nearest


def load_spot(spot_id: str) -> dict:
    """The spot from spots/*.json, with `file`: the name of its spots file, where its region walk starts."""
    spots = {spot["id"]: spot | {"file": path.stem} for path in sorted(SPOTS.glob("*.json"))
             for spot in json.loads(path.read_text())}
    if spot_id not in spots:
        sys.exit(f"unknown spot {spot_id!r}; spots/ has {', '.join(spots)}")
    spot = dict(spots[spot_id])
    photo = spot.get("photo")
    if isinstance(photo, str):
        # A Peak View fixture: position, ground and camera come with the photograph.
        fixture = json.loads((PHOTO_JSON / f"{photo}.json").read_text())
        spot.update(lat=fixture["lat"], lon=fixture["lon"], ground_m=fixture["ground_m"])
        spot["photo"] = {key: fixture[key] for key in
                         ("heading_deg", "pitch_deg", "lens_mm", "subject")}
        spot["photo"]["credit"] = CREDITS["photo"]
        spot["photo_source"] = fixture_photo(photo)
    elif photo:
        # A third-party photograph, kept in the cache only; its camera was aligned in the page.
        suffix = Path(urllib.parse.urlparse(photo["url"]).path).suffix.lower()
        spot["photo_source"] = http_download(photo["url"], SOURCES / "photos" / f"{spot_id}{suffix}",
                                             headers={"User-Agent": USER_AGENT})
        spot["photo"] = {key: value for key, value in photo.items() if key != "url"}
    if spot.get("photo"):
        spot["photo"]["file"] = "photo.jpg"
        spot.update(heading=spot["photo"]["heading_deg"], pitch=spot["photo"]["pitch_deg"])
    return spot


def glo30_tiles(bbox, download: bool) -> list[str]:
    """Every GLO-30 tile the box touches: a local copy if there is one, else the public COG,
    downloaded first when the warp reads it at full resolution."""
    listing = SOURCES / "glo30-tiles.txt"
    if not listing.exists():
        write_atomic(listing, urllib.request.urlopen(f"{GLO30_S3}/tileList.txt", timeout=120).read().decode())
    published = set(listing.read_text().split())
    west, south, east, north = bbox
    tiles = []
    for lat in range(int(np.floor(south)), int(np.floor(north)) + 1):
        for lon in range(int(np.floor(west)), int(np.floor(east)) + 1):
            name = (f"Copernicus_DSM_COG_10_{'N' if lat >= 0 else 'S'}{abs(lat):02d}_00_"
                    f"{'E' if lon >= 0 else 'W'}{abs(lon):03d}_00_DEM")
            local = GLO30_LOCAL / f"{name}.tif"
            cached = SOURCES / "glo30" / f"{name}.tif"
            if local.exists() or cached.exists():
                tiles.append(str(local if local.exists() else cached))
            elif name in published and download:
                with machine_lock("glo30"):
                    tiles.append(str(http_download(f"{GLO30_S3}/{name}/{name}.tif", cached)))
            elif name in published:
                tiles.append(f"/vsicurl/{GLO30_S3}/{name}/{name}.tif")
    return tiles


def warp(inputs, frame: str, half: float, resampling: str, overviews: bool, work: Path,
         threads: bool = True, size: int = SIZE, keep: Path | None = None) -> np.ndarray:
    """Every band of the inputs on level grid `half`, size² texels, as float32 with NODATA; with
    `keep`, the raster also stays there."""
    out = keep or Path(tempfile.mkdtemp(dir=work)) / "warp.tif"
    cmd = ["gdalwarp", "-q", "-overwrite", "-t_srs", frame, "-te", str(-half), str(-half), str(half), str(half),
           "-ts", str(size), str(size), "-r", resampling, "-ot", "Float32", "-dstnodata", str(NODATA)]
    if threads:
        cmd += ["-multi", "-wo", "NUM_THREADS=ALL_CPUS"]
    if not overviews:
        cmd += ["-ovr", "NONE"]
    subprocess.run(cmd + [str(p) for p in inputs] + [str(out)], check=True, env={**os.environ, **GDAL_OPTIONS})
    with rasterio.open(out) as src:
        bands = src.read()
    if not keep:
        shutil.rmtree(out.parent)
    return bands


def glo30_level(k: int, frame: str, bbox, work: Path) -> np.ndarray:
    half = HALF0_M * 2**k
    px = 2 * half / HEIGHT_SIZE
    # Upsampling 30 m cells needs interpolation, cubic so the cells do not show as facets;
    # downsampling keeps each summit with max.
    resampling = "cubic" if px < GLO30_PX_M else "max"
    # COG overviews are averaged, which flattens summits; past k = 5 (78 m texels) that costs
    # under 0.03° and saves reading full-resolution remote tiles.
    heights = warp(glo30_tiles(bbox, download=k <= 5), frame, half, resampling, overviews=k > 5, work=work,
                   size=HEIGHT_SIZE)[0]
    # Missing tiles are sea.
    heights[heights == NODATA] = 0.0
    return heights


def coarse(heights: np.ndarray) -> np.ndarray:
    """Heights on the other layers' grid: the mean of each block of texels."""
    f = heights.shape[0] // SIZE
    return heights.reshape(SIZE, f, SIZE, f).mean(axis=(1, 3))


def fine(layer: np.ndarray) -> np.ndarray:
    """A layer on the heights' grid, each texel repeated."""
    f = HEIGHT_SIZE // SIZE
    return layer.repeat(f, axis=0).repeat(f, axis=1)


def dtm_rasters(key: str, bbox) -> list[Path]:
    if key == "de-bw":
        return bw_rasters(bbox)
    if key == "scot":
        return scot_rasters(bbox)
    if key in BOXED:
        west, south, east, north = bbox
        rows = math.ceil((north - south) * 111_320 / BOX_M)
        cols = math.ceil((east - west) * 111_320 * math.cos(math.radians(south)) / BOX_M)
        boxes = [(west + (east - west) * j / cols, south + (north - south) * i / rows,
                  west + (east - west) * (j + 1) / cols, south + (north - south) * (i + 1) / rows)
                 for i in range(rows) for j in range(cols)]
        return [path for box in boxes for path in BOXED[key].fetch(box, SOURCES / key)]
    return DTM_SOURCES[key].fetch(bbox, SOURCES / key)


def os_square(e: int, n: int) -> str:
    """The OS grid reference of the 10 km square holding British National Grid metres e, n."""
    letters = "ABCDEFGHJKLMNOPQRSTUVWXYZ"
    e100, n100 = e // 100_000, n // 100_000
    first = (19 - n100) - (19 - n100) % 5 + (e100 + 10) // 5
    second = (19 - n100) * 5 % 25 + e100 % 5
    return f"{letters[first]}{letters[second]}{e // 10_000 % 10}{n // 10_000 % 10}"


def scot_rasters(bbox) -> list[Path]:
    """Scotland's LiDAR DTM tiles over the box; squares it never flew are absent."""
    xs, ys = Transformer.from_crs("EPSG:4326", "EPSG:27700", always_xy=True).transform(
        [bbox[0], bbox[2], bbox[0], bbox[2]], [bbox[1], bbox[1], bbox[3], bbox[3]])
    squares = {os_square(e, n) for e in range(int(min(xs)) // 10_000 * 10_000, int(max(xs)) + 1, 10_000)
               for n in range(int(min(ys)) // 10_000 * 10_000, int(max(ys)) + 1, 10_000)}
    paths = [http_download(SCOT_LIDAR.format(name), SOURCES / "scot" / f"{name}.tif", optional=True) for name in sorted(squares)]
    return [path for path in paths if path]


def bw_rasters(bbox) -> list[Path]:
    """Baden-Württemberg's DGM1 as compressed GeoTIFFs.

    A 2 km zip unpacks to 116 MB of XYZ text, so each zip is converted and its text deleted
    before the next. The heights are metres, though the registry row says centimetres.
    """
    out = SOURCES / "de-bw"
    out.mkdir(parents=True, exist_ok=True)

    def one(item) -> list[Path]:
        name, url = item
        done = out / f"{name}.tifs"
        if not done.exists():
            converted = []
            with tempfile.TemporaryDirectory(dir=out) as tmp:
                # A square outside the state is not published.
                archive = http_download(url, Path(tmp) / name, optional=True)
                members = zipfile.ZipFile(archive).namelist() if archive else []
                for member in (m for m in members if m.endswith(".xyz")):
                    xyz = Path(zipfile.ZipFile(archive).extract(member, tmp))
                    tif = out / f"{xyz.stem}.tif"
                    subprocess.run(["gdal_translate", "-q", "-a_srs", "EPSG:25832", "-co", "COMPRESS=DEFLATE",
                                    "-co", "PREDICTOR=3", "-co", "TILED=YES", str(xyz), str(tif)], check=True)
                    xyz.unlink()
                    converted.append(tif.name)
            done.write_text("\n".join(converted))
        return [out / tif for tif in done.read_text().split()]

    with ThreadPoolExecutor(max_workers=4) as pool:
        return [tif for tifs in pool.map(one, DTM_SOURCES["de-bw"].files(bbox)) for tif in tifs]


def worldcover_tiles(bbox) -> list[str]:
    """Every published WorldCover tile the box touches. A tile wholly over the sea does not exist."""
    known_path = SOURCES / "worldcover-tiles.json"
    west, south, east, north = bbox
    tiles = []
    with machine_lock("worldcover"):
        known = json.loads(known_path.read_text()) if known_path.exists() else {}
        for lat in range(math.floor(south / 3) * 3, math.floor(north / 3) * 3 + 1, 3):
            for lon in range(math.floor(west / 3) * 3, math.floor(east / 3) * 3 + 1, 3):
                url = WORLDCOVER.format(f"{'N' if lat >= 0 else 'S'}{abs(lat):02d}{'E' if lon >= 0 else 'W'}{abs(lon):03d}")
                if url not in known:
                    try:
                        urllib.request.urlopen(urllib.request.Request(url, method="HEAD"), timeout=60)
                        known[url] = True
                    except urllib.error.HTTPError:
                        known[url] = False
                if known[url]:
                    tiles.append(f"/vsicurl/{url}")
        write_atomic(known_path, json.dumps(known, indent=1))
    return tiles


def material_level(k: int, frame: str, bbox, heights: np.ndarray, work: Path, coast: list[np.ndarray] | None = None,
                   wet: np.ndarray | None = None) -> tuple[np.ndarray, np.ndarray | None]:
    """R8: the material of WorldCover's class, sea and inland water as they settle; and the sea
    from OSM's coastline, or None. With OSM's water coverage (`wet`), water settles where it lies."""
    half = HALF0_M * 2**k
    resampling = "near" if 2 * half / SIZE <= WORLDCOVER_PX_M else "mode"
    raw = warp(worldcover_tiles(bbox), frame, half, resampling, overviews=True, work=work)[0]
    lookup = np.full(256, NONE, np.uint8)
    for value, name in WORLDCOVER_MATERIALS.items():
        lookup[value] = MATERIAL[name]
    material = lookup[np.where(raw == NODATA, 0, raw).astype(np.uint8)]
    # No tile is published over open sea.
    material[(material == NONE) & (heights <= 0.5)] = WATER
    material = round_shores(material, half)
    sea = sea_mask(coast, material == WATER, half) if coast else None
    if wet is not None:
        material = settle_water(material, heights, sea, wet, half)
    material[(material == WATER) & (sea if sea is not None else heights <= SEA_LEVEL_M)] = SEA
    return material, sea


def is_water(material: np.ndarray) -> np.ndarray:
    return (material == WATER) | (material == SEA)


def settle_water(material: np.ndarray, heights: np.ndarray, sea: np.ndarray | None, wet: np.ndarray,
                 half: float) -> np.ndarray:
    """The sea lies where OSM's coastline puts it, to the texel. Other WorldCover water stays where
    OSM maps water, or where the ground is flat as a lake is, away from the sea's shore; elsewhere
    it was a shadow or a gully finer than the DEM, and takes the nearest land's material. Without a
    coastline, water at sea level and the water joined to it is the sea."""
    material = material.copy()
    water = material == WATER
    # The spread is taken over water only, because the DEM blends the bank into a lake's edge;
    # water joined to a flat surface is the same lake. A lone texel is a shadow.
    surface = grow(np.where(water, heights, -np.inf), np.maximum) - grow(np.where(water, heights, np.inf), np.minimum)
    flat = water & (surface < FLAT_WATER_M) & (box_mean(water.astype(np.float32), 1) > 0.3)
    for _ in range(2):
        flat |= grow(flat, np.logical_or) & water
    if sea is None:
        # The sea is water at sea level and the water joined to it, which GLO-30 smears up a cliff.
        keep = water & (heights <= SEA_LEVEL_M)
        for _ in range(round(2 * COAST_SHORE_M / (2 * half / SIZE))):
            keep |= grow(keep, np.logical_or) & water
        keep |= (wet >= 0.5) | flat
    else:
        steps = max(1, round(COAST_SHORE_M / (2 * half / SIZE)))
        shore = sea.copy()
        for _ in range(steps):
            shore = grow(shore, np.logical_or)
        keep = sea | (wet >= 0.5) | (flat & ~shore)
        # A river or lagoon keeps its width across the shore band; water joined only to the sea
        # does not.
        for _ in range(steps):
            keep |= grow(keep & ~sea, np.logical_or) & water & ~sea
        material[sea] = WATER
    # Land that no land material reaches, as an islet or a mole OSM maps in the sea, is sparse.
    return spread(material, water & ~keep, MATERIAL["sparse"], avoid=(material == NONE) | (material == WATER))


def spread(index: np.ndarray, lost: np.ndarray, otherwise: int, avoid: np.ndarray | None = None,
           steps: int | None = None) -> np.ndarray:
    """Lost texels take the value of the nearest texel that is neither lost nor in `avoid`,
    spreading in from their edges; those it does not reach within `steps` take `otherwise`. Shifted
    by slicing, so no texel sees the far edge."""
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


def round_shores(material: np.ndarray, half: float) -> np.ndarray:
    """Finer than WorldCover's pixels, its shores are stair-stepped: water by the majority within
    half a pixel instead, and land that loses its water takes the nearest land's material."""
    r = round(WORLDCOVER_PX_M / 2 / (2 * half / SIZE))
    if not r:
        return material
    material = material.copy()
    water = box_mean((material == WATER).astype(np.float32), r) > 0.5
    lost = (material == WATER) & ~water
    material[water] = WATER
    return spread(material, lost, MATERIAL["grassland"], avoid=(material == NONE) | water)


def s2_items(bbox, work: Path) -> dict[int, list[Path]]:
    """Per quarter, one five-band VRT per mosaic tile the box touches, read over CDSE S3."""
    url = (f"{S2_STAC}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}"
           f"&datetime={S2_YEAR}-01-01T00:00:00Z/{S2_YEAR}-12-31T23:59:59Z&limit=100")
    quarters = {q: [] for q in range(1, 5)}
    for item in stac_items(url):
        quarter = (int(item["properties"]["start_datetime"][5:7]) - 1) // 3 + 1
        vrt = work / f"{item['id']}.vrt"
        bands = [item["assets"][band]["href"].replace("s3://", "/vsis3/") for band in S2_BANDS]
        subprocess.run(["gdalbuildvrt", "-q", "-separate", str(vrt), *bands], check=True,
                       env={**os.environ, **GDAL_OPTIONS})
        quarters[quarter].append(vrt)
    return quarters


def s2_level(k: int, frame: str, quarters: dict[int, list[Path]], work: Path) -> dict[int, np.ndarray]:
    """RGBA8 per quarter: visible reflectance as refl^(1/2.2), and NDVI as 1–255, 0 where looks
    are few. Winter pixels in terrain shadow take their chroma from summer."""
    half = HALF0_M * 2**k
    resampling = "bilinear" if 2 * half / SIZE < 10 else "average"

    def read(vrts: list[Path]) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        b02, b03, b04, b08, looks = warp(vrts, frame, half, resampling, overviews=True, work=work, threads=False)
        valid = (looks >= MIN_OBSERVATIONS) & (b04 != NODATA)
        visible = np.clip(np.stack([b04, b03, b02]) / 10000.0, 0.0, 1.0)
        ndvi = np.clip((b08 - b04) / np.maximum(b08 + b04, 1.0), -1.0, 1.0)
        return visible, ndvi, valid

    # CDSE allows 4 connections per user: the four quarters read side by side, each on one thread.
    with ThreadPoolExecutor(len(quarters)) as pool:
        seen = dict(zip(quarters, pool.map(read, quarters.values())))
    summer = seen[3]
    for q in (1, 4):
        visible, ndvi, valid = seen[q]
        luma = visible.mean(axis=0)
        shaded_snow = (visible[2] > 1.15 * visible[0]) & (ndvi < 0.1)
        dark = valid & summer[2] & (luma < DARK) & ~shaded_snow
        scale = luma / np.maximum(summer[0].mean(axis=0), 1e-4)
        visible[:, dark] = (summer[0] * scale)[:, dark]
    texels = {}
    for q, (visible, ndvi, valid) in seen.items():
        rgb = np.round(255.0 * visible ** (1 / 2.2))
        a = np.where(valid, 1 + np.round((np.maximum(ndvi, -0.99) + 1) / 2 * 254), 0)
        texels[q] = np.stack([*rgb, a], axis=-1).astype(np.uint8)
    return texels


def modis_snow(bbox) -> Path:
    """The MODIS season planes of the box, snapped out to MODIS_SNAP_DEG, on the MODIS grid: a GeoTIFF of
    an onset and a melt-out band per season in _sources/snow/, read once as the planner reads them."""
    snap = lambda value, rounding: rounding(value / MODIS_SNAP_DEG) * MODIS_SNAP_DEG
    box = (snap(bbox[0], math.floor), snap(bbox[1], math.floor), snap(bbox[2], math.ceil), snap(bbox[3], math.ceil))
    path = SOURCES / "snow" / f"modis-{'_'.join(f'{v:g}' for v in box)}-{FIRST_SEASON}-{LAST_SEASON}.tif"
    with machine_lock("modis"):
        if path.exists():
            return path
        groups = (items for _, items in planner_snow.modis_seasons(box, FIRST_SEASON, LAST_SEASON))
        planes, grid = planner_snow.modis_planes(groups, box, FIRST_SEASON, LAST_SEASON, hansen_tiles(box))
        part = path.with_name(path.name + ".part")
        part.parent.mkdir(parents=True, exist_ok=True)
        with rasterio.open(part, "w", driver="GTiff", width=grid.shape[1], height=grid.shape[0], count=2 * len(SNOW_SEASONS),
                           dtype="uint8", crs=grid.crs, transform=grid.transform, nodata=planner_snow.NO_DATA,
                           compress="deflate") as dst:
            dst.write(planes.reshape(-1, *grid.shape))
        os.replace(part, path)
    return path


def hansen_tiles(bbox) -> list[str]:
    """The local Hansen GFC tree canopy rasters of the box, downloaded once into _sources/hansen/."""
    version = planner_sources.VERSIONS["hansen-gfc"]
    with machine_lock("hansen"):
        return [str(http_download(planner_snow.CANOPY.format(version, tile), SOURCES / "hansen" / f"{version}-{tile}.tif"))
                for tile in planner_snow.canopy_tiles(bbox)]


def texel_centres(k: int, frame: str, crs) -> tuple[np.ndarray, np.ndarray]:
    """The level's texel centres in `crs`, row 0 north."""
    half = HALF0_M * 2**k
    centres = (np.arange(SIZE) + 0.5) * 2 * half / SIZE - half
    x, y = np.meshgrid(centres, -centres)
    return Transformer.from_crs(CRS.from_proj4(frame), crs, always_xy=True).transform(x.ravel(), y.ravel())


def modis_level(k: int, frame: str, modis: Path) -> np.ndarray:
    """The MODIS planes (seasons, 2, SIZE, SIZE) of level k: the blend of the four pixels around each texel
    centre with bilinear weights, as planner_snow.sample takes them at its max zoom."""
    with rasterio.open(modis) as src:
        planes = src.read().reshape(len(SNOW_SEASONS), 2, src.height, src.width)
        transform, crs = src.transform, src.crs
    col, row = ~transform @ texel_centres(k, frame, crs)
    col, row = col - 0.5, row - 0.5
    c0, r0 = np.floor(col).astype(np.int64), np.floor(row).astype(np.int64)
    fc, fr = col - c0, row - r0
    rows, cols = planes.shape[2:]
    values, weights = [], []
    for dr, dc, weight in ((0, 0, (1 - fr) * (1 - fc)), (0, 1, (1 - fr) * fc), (1, 0, fr * (1 - fc)), (1, 1, fr * fc)):
        r, c = r0 + dr, c0 + dc
        inside = (r >= 0) & (r < rows) & (c >= 0) & (c < cols)
        values.append(np.where(inside, planes[:, :, np.clip(r, 0, rows - 1), np.clip(c, 0, cols - 1)], planner_snow.NO_DATA))
        weights.append(weight)
    return planner_snow.blend(np.stack(values), np.stack(weights).astype(np.float32)).reshape(len(SNOW_SEASONS), 2, SIZE, SIZE)


def hr_wsi_level(k: int, frame: str, to_wgs84, work: Path) -> np.ndarray:
    """The HR-WSI planes (seasons, 2, SIZE, SIZE) of level k from the Snow Phenology products over CDSE S3.
    A season without a product is no data."""
    half = HALF0_M * 2**k
    f = SNOW_OVERSAMPLE
    files = planner_snow.copernicus_files(to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21))
    planes = np.full((len(SNOW_SEASONS), 2, SIZE, SIZE), planner_snow.NO_DATA, np.uint8)

    def season(s: int) -> None:
        layers = files.get(SNOW_SEASONS[s], {})
        vrts = []
        for paths in zip(*(layers.get(name, []) for name in ("SCO", "SCM", "SCD"))):
            vrt = work / f"{Path(paths[0]).stem}.vrt"
            if not vrt.exists():
                subprocess.run(["gdalbuildvrt", "-q", "-separate", str(vrt), *paths], check=True,
                               env={**os.environ, **GDAL_OPTIONS})
            vrts.append(vrt)
        if not vrts:
            return
        bands = warp(vrts, frame, half, "near", overviews=True, work=work, threads=False, size=f * SIZE)
        days = planner_snow.season_start(SNOW_SEASONS[s], SNOW_SEASONS[s] + 1)
        weights = np.full((f * f, SNOW_STRIP * SIZE), 1 / f**2, np.float32)
        # In strips of texel rows, so a season holds little more than its warp.
        for top in range(0, SIZE, SNOW_STRIP):
            # 65535 is no data and 420 inland water, as copernicus_planes reads them.
            strip = bands[:, top * f:(top + SNOW_STRIP) * f]
            onset, meltout, duration = (np.where(band == NODATA, 65535, band).astype(np.int32) for band in strip)
            data = (duration <= 366) & ((duration == 0) | (onset <= 366) & (meltout <= 366))
            length = np.where(duration == 0, 0, meltout - onset + 1)
            fine = planner_snow.encode(onset, length, days, data)
            blocks = fine.reshape(2, SNOW_STRIP, f, SIZE, f).transpose(2, 4, 0, 1, 3).reshape(f * f, 1, 2, SNOW_STRIP * SIZE)
            planes[s, :, top:top + SNOW_STRIP] = planner_snow.blend(blocks, weights)[0].reshape(2, SNOW_STRIP, SIZE)

    # Two seasons at a time: each warp holds about 200 MB, and the box is shared.
    with ThreadPoolExecutor(2) as pool:
        list(pool.map(season, range(len(SNOW_SEASONS))))
    return planes


def smoothstep(low: float, high: float, x: np.ndarray) -> np.ndarray:
    t = np.clip((x - low) / (high - low), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def snow_free(quarters: dict[int, np.ndarray]) -> dict[int, np.ndarray]:
    """The quarters (RGBA8, as s2_level writes them) with no snow in their colour: the snow data says
    where snow lies, and the satellite only gives bare ground its colour. A texel that a quarter sees
    snow on takes the colour of the nearest quarter that does not, the previous one first, or black
    where none does, and the alpha SNOW_MARK, so that the page can still read the satellite's snow
    where the snow data has none. Run again, it changes nothing."""
    def snowy(texels: np.ndarray) -> np.ndarray:
        # The page's snow test: as blue as it is red, and bright or in shade where nothing grows.
        a = texels[..., 3].astype(np.float32)
        refl = (texels[..., :3] / 255.0) ** 2.2
        red, blue = refl[..., 0], refl[..., 2]
        bright = smoothstep(0.36, 0.5, refl.min(axis=-1)) * smoothstep(0.9, 0.97, blue / np.maximum(red, 1e-3))
        shaded = (blue >= 1.15 * red) * ((a - 1) / 127 - 1 <= 0.1) * smoothstep(0.03, 0.06, blue)
        return (a == SNOW_MARK) | ((a > SNOW_MARK) & (np.maximum(bright, shaded) > 0.5))

    snow = {q: snowy(texels) for q, texels in quarters.items()}
    clean = {}
    for q, texels in quarters.items():
        texels = texels.copy()
        need = snow[q].copy()
        for other in ((q - 2) % 4 + 1, q % 4 + 1, (q + 1) % 4 + 1):
            take = need & (quarters[other][..., 3] > SNOW_MARK) & ~snow[other]
            texels[take, :3] = quarters[other][take, :3]
            need &= ~take
        texels[need, :3] = 0
        texels[snow[q], 3] = SNOW_MARK
        clean[q] = texels
    return clean


def unpacked(path: Path, bands: int) -> np.ndarray:
    return np.frombuffer(gzip.decompress(path.read_bytes()), np.uint8).reshape(SIZE, SIZE, bands)


def satellite_heath(quarters: dict[int, np.ndarray], most: np.ndarray) -> np.ndarray:
    """Where the satellite sees heath: in summer's quarter, or spring's where summer saw nothing,
    the ground reflects as much red as green while its NDVI is high and near the year's most."""
    texels = np.where((quarters[3][..., 3] > 0)[..., None], quarters[3], quarters[2])
    refl = (texels[..., :3] / 255.0) ** 2.2
    ndvi = (texels[..., 3] - 1.0) / 127 - 1
    return ((texels[..., 3] > 0) & (refl[..., 1] < MOOR_GREEN_RED * refl[..., 0]) & (ndvi >= MOOR_NDVI)
            & (ndvi >= (most - 1.0) / 127 - 1 - MOOR_PEAK))


def refine(material: np.ndarray, proposed: np.ndarray) -> np.ndarray:
    """Each proposed material where it names the texel's material more precisely (REFINES); 0
    proposes nothing."""
    out = material.copy()
    for name, coarser in REFINES.items():
        at = (proposed == MATERIAL[name]) & np.isin(material, [MATERIAL[c] for c in coarser])
        out[at] = MATERIAL[name]
    return out


def overlay(material: np.ndarray, proposed: np.ndarray) -> np.ndarray:
    """A finer map's material on every land texel it names: WorldCover and the satellite keep the water."""
    at = (proposed != NONE) & ~is_water(material)
    out = material.copy()
    out[at] = proposed[at]
    return out


def polygons_level(polygons: list[tuple[list[np.ndarray], int]], half: float) -> np.ndarray:
    """R8: the code of the polygon over each texel, 0 under none. A polygon is its rings in frame
    metres, its holes among them; the smaller burns over the larger."""
    from rasterio.features import rasterize

    if not polygons:
        return np.zeros((SIZE, SIZE), np.uint8)
    size = lambda rings: max(abs(float(np.sum(r[:-1, 0] * r[1:, 1] - r[1:, 0] * r[:-1, 1]))) for r in rings)
    # GDAL fills a polygon even-odd over all its rings, so a multipolygon's outers and inners can share one.
    shapes = [({"type": "Polygon", "coordinates": [r.tolist() for r in rings]}, code)
              for rings, code in sorted(polygons, key=lambda p: -size(p[0]))]
    return rasterize(shapes, out_shape=(SIZE, SIZE), dtype=np.uint8,
                     transform=rasterio.transform.from_bounds(-half, -half, half, half, SIZE, SIZE))


def clc_legend() -> dict[int, int]:
    """The CLC raster's legend as 0xRRGGBB → class, from the service's swatches, cached."""
    path = SOURCES / "clc" / "legend.json"
    if not path.exists():
        legend = json.loads(urllib.request.urlopen(f"{CLC[0]}/legend?f=json", timeout=120).read())
        colours = {}
        for entry in next(layer for layer in legend["layers"] if layer["layerId"] == 1)["legend"]:
            with rasterio.MemoryFile(base64.b64decode(entry["imageData"])) as file, file.open() as image:
                swatch = image.read()
            r, g, b = (int(v) for v in swatch[:3, swatch.shape[1] // 2, swatch.shape[2] // 2])
            colours[(r << 16) | (g << 8) | b] = int(entry["label"].split(":")[0])
        write_atomic(path, json.dumps(colours))
    return {int(rgb): code for rgb, code in json.loads(path.read_text()).items()}


def clc_level(spot_id: str, lat: float, lon: float, k: int, frame: str, work: Path) -> np.ndarray:
    """R8 on level k: the material that each texel's CLC class names (CLC_MATERIALS), 0 elsewhere
    and outside CLC. One map image per spot and level in EPSG:3035, at 100 m or the service's
    4096 px, cached."""
    half = HALF0_M * 2**k
    west, south, east, north = Transformer.from_crs(CRS.from_proj4(frame), "EPSG:3035", always_xy=True).transform_bounds(
        -half, -half, half, half, densify_pts=21)
    px = max(east - west, north - south) / min(4096, math.ceil(max(east - west, north - south) / CLC_PX_M))
    w, h = math.ceil((east - west) / px), math.ceil((north - south) / px)
    # The box widened to whole pixels, so the service draws it at exactly this size.
    x0, y0 = (west + east - w * px) / 2, (south + north - h * px) / 2
    bounds = (x0, y0, x0 + w * px, y0 + h * px)
    path = SOURCES / "clc" / f"{spot_id}_{lat:.5f}_{lon:.5f}-{k}.png"
    if not path.exists():
        params = {"bbox": ",".join(f"{v:.1f}" for v in bounds), "bboxSR": 3035, "imageSR": 3035, "size": f"{w},{h}",
                  "layers": "show:1", "format": "png32", "transparent": "true", "f": "image"}
        with machine_lock("clc"):
            http_download(f"{CLC[0]}/export?{urllib.parse.urlencode(params)}", path, headers={"User-Agent": USER_AGENT})
    with rasterio.open(path) as f:
        rgba = f.read().astype(np.int64)
    colour = np.where(rgba[3] > 0, (rgba[0] << 16) | (rgba[1] << 8) | rgba[2], -1)
    lookup = {rgb: MATERIAL[CLC_MATERIALS[code]] for rgb, code in clc_legend().items() if code in CLC_MATERIALS}
    materials = np.zeros(colour.shape, np.uint8)
    for rgb, code in lookup.items():
        materials[colour == rgb] = code
    return onto_level(materials[None], bounds, "EPSG:3035", k, frame, "mode" if 2 * half / SIZE > px else "near", work)[0]


def lithology_sources() -> tuple[Path, Path]:
    """EGDI's GeoPackage and IGME 5000's shapefile, each downloaded once."""
    folder = SOURCES / "lithology"
    shapefile = folder / "igme5000" / f"{IGME5000[1]}.shp"
    with machine_lock("lithology"):
        egdi = http_download(EGDI[0], folder / "GeologicUnitView.gpkg")
        if not shapefile.exists():
            archive = http_download(IGME5000[0], folder / "IGME5000.zip")
            with zipfile.ZipFile(archive) as z:
                for member in z.namelist():
                    if Path(member).stem == IGME5000[1]:
                        (shapefile.parent / Path(member).name).write_bytes(z.read(member))
            archive.unlink()
    return egdi, shapefile


def lithology_level(sources: tuple[Path, Path], k: int, frame: str, work: Path, used: set) -> np.ndarray:
    """R8 on level k: the lithology of each texel from EGDI, and from IGME 5000 where EGDI names
    none. Each map's units over the level, simplified to half a texel, are burnt onto it; the
    credit of each map that names any goes into `used`."""
    from rasterio.features import rasterize

    half = HALF0_M * 2**k
    # Both maps are in EPSG:3034, so the filter box is in their own metres.
    box = Transformer.from_crs(CRS.from_proj4(frame), "EPSG:3034", always_xy=True).transform_bounds(
        -half, -half, half, half, densify_pts=21)
    out = np.zeros((SIZE, SIZE), np.uint8)
    for path, (_, layer, field, credit), terms in zip(sources, (EGDI, IGME5000), (EGDI_TERMS, IGME5000_TERMS)):
        codes = {term: LITHOLOGY[name] for name, words in terms.items() for term in words.split("|")}
        units = work / "units.geojson"
        units.unlink(missing_ok=True)
        subprocess.run(["ogr2ogr", "-q", "-f", "GeoJSON", str(units), str(path), layer, "-select", field,
                        "-spat", *map(str, box), "-t_srs", frame, "-simplify", str(half / SIZE), "-nlt", "MULTIPOLYGON"],
                       check=True)
        shapes = [(f["geometry"], codes[term]) for f in json.loads(units.read_text())["features"]
                  if f["geometry"] and (term := (f["properties"][field] or "").rsplit("/", 1)[-1]) in codes]
        if shapes:
            unknown = out == 0
            out[unknown] = rasterize(shapes, out_shape=(SIZE, SIZE), dtype=np.uint8,
                                     transform=rasterio.transform.from_bounds(-half, -half, half, half, SIZE, SIZE))[unknown]
            used.add(credit)
    return out


def road_surface(tags: dict) -> str | None:
    """Which painted surface a highway gets, or None for one that is not on the ground."""
    highway, surface, grade = tags["highway"], tags.get("surface", ""), tags.get("tracktype", "")
    if tags.get("tunnel") in ("yes", "building_passage") or highway in ("proposed", "construction", "platform"):
        return None
    if surface in ("asphalt", "paved", "concrete", "paving_stones", "sett", "chipseal"):
        return "main" if highway in ("motorway", "trunk", "primary", "secondary") else "road"
    if surface in ("gravel", "compacted", "fine_gravel", "pebblestone"):
        return "gravel"
    if surface in ("dirt", "ground", "earth", "mud", "unpaved", "sand"):
        return "dirt"
    if surface in ("grass", "grass_paver"):
        return "grass"
    if highway in ("path", "footway", "bridleway", "steps", "pedestrian", "via_ferrata"):
        return "path"
    if highway == "track":
        return {"grade1": "road", "grade2": "gravel", "grade3": "dirt"}.get(grade, "grass")
    if highway in ("motorway", "trunk", "primary", "secondary", "motorway_link", "trunk_link", "primary_link"):
        return "main"
    return "road"


def road_width(tags: dict, surface: str) -> float:
    """The half-width a road is drawn at: OSM's width where it gives one."""
    try:
        return max(float(tags.get("width", "").split()[0]) / 2, 0.4)
    except (ValueError, IndexError):
        paved = surface in ("main", "road")
        return PAVED_WIDTHS.get(tags["highway"], ROAD_WIDTHS[surface]) if paved else ROAD_WIDTHS[surface]


def edge_distance(lines: list[tuple[np.ndarray, float]], half: float, size: int, reach: float):
    """Over a size² grid on ±half: the distance to the nearest edge of the lines, each drawn at
    its half-width, negative inside and at most `reach`; that line's index + 1, 0 for none; and how
    far along it the nearest point lies, in metres."""
    px = 2 * half / size
    distance = np.full((size, size), reach, np.float32)
    nearest = np.zeros((size, size), np.int32)
    along = np.zeros((size, size), np.float32)
    for n, (xy, width) in enumerate(lines, 1):
        start = 0.0
        for (ax, ay), (bx, by) in zip(xy[:-1], xy[1:]):
            length = float(np.hypot(bx - ax, by - ay))
            start += length
            i0 = max(int((min(ax, bx) - width - reach + half) / px), 0)
            i1 = min(int((max(ax, bx) + width + reach + half) / px) + 1, size)
            j0 = max(int((half - max(ay, by) - width - reach) / px), 0)
            j1 = min(int((half - min(ay, by) + width + reach) / px) + 1, size)
            if i0 >= i1 or j0 >= j1:
                continue
            x, y = np.meshgrid(-half + (np.arange(i0, i1) + 0.5) * px, half - (np.arange(j0, j1) + 0.5) * px)
            dx, dy = bx - ax, by - ay
            t = np.clip(((x - ax) * dx + (y - ay) * dy) / max(dx * dx + dy * dy, 1e-9), 0, 1)
            d = np.hypot(x - ax - t * dx, y - ay - t * dy) - width
            closer = d < distance[j0:j1, i0:i1]
            distance[j0:j1, i0:i1][closer] = d[closer]
            nearest[j0:j1, i0:i1][closer] = n
            along[j0:j1, i0:i1][closer] = start - length * (1 - t[closer])
    return distance, nearest, along


def road_field(roads: list[tuple[np.ndarray, int, float, bool]], half: float) -> np.ndarray:
    """RGB8: (distance to the nearest road edge + 5 m) in 0.1 m steps, negative inside a road; that
    road's surface code; and its half-width in 0.05 m steps, so the page knows where its middle is.
    A bridge drawn as a deck comes with code 0, which the page does not paint."""
    distance, nearest, _ = edge_distance([(xy, width) for xy, _, width, _ in roads], half, SIZE, ROAD_REACH_M)
    surface = np.array([0] + [code for _, code, _, _ in roads], np.uint8)[nearest]
    width = np.array([0] + [min(round(width * 20), 255) for _, _, width, _ in roads], np.uint8)[nearest]
    return np.stack([np.round((distance + 5) * 10).astype(np.uint8), surface, width], axis=-1)


def passes_within(xy: np.ndarray, r: float) -> bool:
    """Whether the line passes within r metres of the eye."""
    a, d = xy[:-1], np.diff(xy, axis=0)
    t = np.clip(-(a * d).sum(1) / np.maximum((d * d).sum(1), 1e-9), 0, 1)
    return bool(np.hypot(*(a + t[:, None] * d).T).min() < r) if len(d) else False


def chain_bridges(bridges: list[tuple[np.ndarray, int, float]]) -> list[tuple[np.ndarray, int, float]]:
    """Bridges OSM splits into several ways, joined where an end meets an end, so each deck spans
    bank to bank and only the banks set its level."""
    out = list(bridges)
    i = 0
    while i < len(out):
        for j in range(i + 1, len(out)):
            a, b = out[i][0], out[j][0]
            ends = [(a, b), (a, b[::-1]), (a[::-1], b), (a[::-1], b[::-1])]
            joined = next((np.concatenate([x, y[1:]]) for x, y in ends if np.hypot(*(x[-1] - y[0])) < 1.0), None)
            if joined is not None:
                out[i] = (joined, out[i][1], max(out[i][2], out[j][2]))
                del out[j]
                break
        else:
            i += 1
    return out


def grade_roads(heights: np.ndarray, roads: list[tuple[np.ndarray, int, float, bool]], half: float) -> np.ndarray:
    """Heights with the roads built into them. A road lies level across at its centreline's
    height, which is smoothed along it, and the ground beside it is cut or filled to it at no more
    than GRADE_BANK. Bridges keep the ground under them."""
    size = heights.shape[0]
    px = 2 * half / size
    lines = [(xy, width) for xy, _, width, bridge in roads if not bridge]
    distance, nearest, along = edge_distance(lines, half, size, ROAD_REACH_M)
    near = np.flatnonzero(nearest)
    near = near[np.argsort(nearest.flat[near], kind="stable")]
    groups = np.split(near, np.flatnonzero(np.diff(nearest.flat[near])) + 1)
    centre = np.empty(near.size, np.float32)
    ends = np.cumsum([0] + [len(g) for g in groups])
    r = max(1, round(GRADE_SMOOTH_M / px))
    for group, start, end in zip(groups, ends[:-1], ends[1:]):
        if not len(group):
            continue
        xy = lines[nearest.flat[group[0]] - 1][0]
        s = np.concatenate([[0], np.cumsum(np.hypot(*np.diff(xy, axis=0).T))])
        steps = np.linspace(0, s[-1], max(2, int(s[-1] / px) + 1))
        x, y = np.interp(steps, s, xy[:, 0]), np.interp(steps, s, xy[:, 1])
        u = np.clip((x + half) / px - 0.5, 0, size - 1.001)
        v = np.clip((half - y) / px - 0.5, 0, size - 1.001)
        i, j = u.astype(int), v.astype(int)
        fu, fv = u - i, v - j
        z = ((heights[j, i] * (1 - fu) + heights[j, i + 1] * fu) * (1 - fv)
             + (heights[j + 1, i] * (1 - fu) + heights[j + 1, i + 1] * fu) * fv)
        z = np.convolve(np.pad(z, r, mode="edge"), np.ones(2 * r + 1) / (2 * r + 1), mode="valid")
        centre[start:end] = np.interp(along.flat[group], steps, z)
    h = heights.flat[near]
    bank = GRADE_BANK * np.maximum(distance.flat[near], 0)
    graded = centre + np.clip(h - centre, -bank, bank)
    # Back to the ground as it is toward the field's reach, so no seam shows where it ends.
    fade = np.clip((ROAD_REACH_M - distance.flat[near]) / 4, 0, 1)
    out = heights.copy()
    out.flat[near] = h + (graded - h) * fade
    return out


def sea_mask(coast: list[np.ndarray], worldcover_water: np.ndarray, half: float) -> np.ndarray | None:
    """The sea on a level from OSM's coastline, which has the land on its left. The line, drawn
    through every texel it touches, splits the level into regions; each region is sea or land by
    a vote of points just right and just left of every segment, and a region the votes split (a
    gully or neck finer than a texel) keeps WorldCover's water. The line's own texels go with the
    side most of their neighbours took, WorldCover's on a tie. None where no coastline crosses the level, or where the
    sea it gives is mostly land to WorldCover, as a line drawn the wrong way round would flood
    the land."""
    from rasterio.features import rasterize, shapes

    transform = rasterio.transform.from_bounds(-half, -half, half, half, SIZE, SIZE)
    lines = [({"type": "LineString", "coordinates": xy.tolist()}, 1) for xy in coast if len(xy) > 1]
    line = rasterize(lines, out_shape=(SIZE, SIZE), transform=transform, all_touched=True, dtype=np.uint8).astype(bool) \
        if lines else np.zeros((SIZE, SIZE), bool)
    if not line.any():
        return None
    regions = [geometry for geometry, _ in shapes((~line).astype(np.uint8), mask=~line, connectivity=8, transform=transform)]
    label = rasterize([(g, n) for n, g in enumerate(regions, 1)], out_shape=(SIZE, SIZE), transform=transform, dtype=np.int32)
    px = 2 * half / SIZE
    votes = np.zeros((2, len(regions) + 1))
    for xy in coast:
        d = np.diff(xy, axis=0)
        length = np.hypot(*d.T)
        ok = length > 1e-6
        mid, right = ((xy[:-1] + xy[1:]) / 2)[ok], (np.stack([d[:, 1], -d[:, 0]], 1) / np.maximum(length, 1e-6)[:, None])[ok]
        for side, sign in ((0, 1), (1, -1)):
            p = mid + sign * 0.75 * px * right
            i, j = np.floor((p[:, 0] + half) / px).astype(int), np.floor((half - p[:, 1]) / px).astype(int)
            inside = (i >= 0) & (i < SIZE) & (j >= 0) & (j < SIZE)
            np.add.at(votes[side], label[j[inside], i[inside]], 1)
    sea_votes, land_votes = votes
    water_share = np.bincount(label.ravel(), weights=worldcover_water.ravel(), minlength=len(regions) + 1) / \
        np.maximum(np.bincount(label.ravel(), minlength=len(regions) + 1), 1)
    is_sea = np.where(sea_votes > 2 * land_votes, True, np.where(land_votes > 2 * sea_votes, False, water_share > 0.5))
    is_sea[0] = False
    sea = is_sea[label] & ~line
    neighbours = box_mean(sea.astype(np.float32), 1), box_mean((~sea & ~line).astype(np.float32), 1)
    sea |= line & ((neighbours[0] > neighbours[1]) | ((neighbours[0] == neighbours[1]) & worldcover_water))
    if (sea & ~worldcover_water).sum() > 0.3 * max(sea.sum(), 1):
        return None
    return sea


def osm_wet(rivers, lakes, islands, half: float) -> np.ndarray:
    """The share of each texel under OSM's rivers, canals and water areas, less their islands,
    from a 4× finer grid."""
    from rasterio.features import rasterize
    from rasterio.transform import from_origin

    fine = SIZE * SUPERSAMPLE
    reach = 2 * half / fine
    wet = edge_distance(rivers, half, fine, reach)[0] < 0
    if lakes:
        shapes = [({"type": "Polygon", "coordinates": [ring.tolist()]}, value)
                  for rings, value in ((lakes, 1), (islands, 0)) for ring in rings]
        wet |= rasterize(shapes, out_shape=(fine, fine), transform=from_origin(-half, half, reach, reach),
                         dtype=np.uint8).astype(bool)
    return wet.reshape(SIZE, SUPERSAMPLE, SIZE, SUPERSAMPLE).mean(axis=(1, 3))


def water_field(wet: np.ndarray, streams, material: np.ndarray, half: float, work: Path) -> np.ndarray:
    """RG8: the share of each pixel under OSM water (`osm_wet`, plus streams), 0–255, and its shore
    distance (SHORE_REACH_M). Streams count only off woodland, because the canopy hides them."""
    fine = SIZE * SUPERSAMPLE
    reach = 2 * half / fine
    water = wet
    if streams:
        cover = (edge_distance(streams, half, fine, reach)[0] < 0).reshape(SIZE, SUPERSAMPLE, SIZE, SUPERSAMPLE).mean(axis=(1, 3))
        water = np.maximum(water, cover * ~WOODLAND[material])
    shore = shore_distance(is_water(material) | (water >= 0.5), half, work)
    return np.stack([np.round(water * 255), np.round(np.sqrt(shore / SHORE_REACH_M) * 255)], axis=-1).astype(np.uint8)


def shore_distance(water: np.ndarray, half: float, work: Path) -> np.ndarray:
    """Metres from each water texel to the nearest land texel, at most SHORE_REACH_M; 0 on land."""
    src, dst = work / "land.tif", work / "shore.tif"
    with rasterio.open(src, "w", driver="GTiff", height=SIZE, width=SIZE, count=1, dtype="uint8",
                       transform=rasterio.transform.from_bounds(-half, -half, half, half, SIZE, SIZE)) as f:
        f.write((~water).astype(np.uint8)[None])
    subprocess.run([PROXIMITY, "-q", str(src), str(dst), "-values", "1", "-distunits", "GEO", "-ot", "Float32",
                    "-maxdist", str(SHORE_REACH_M), "-nodata", str(SHORE_REACH_M)], check=True)
    with rasterio.open(dst) as f:
        return np.minimum(f.read(1), SHORE_REACH_M) * water


def grow(a: np.ndarray, op) -> np.ndarray:
    """One 3×3 step of a filter: np.maximum for a max filter, np.logical_or for a dilation."""
    out = a.copy()
    out[1:] = op(out[1:], a[:-1])
    out[:-1] = op(out[:-1], a[1:])
    rows = out.copy()
    out[:, 1:] = op(out[:, 1:], rows[:, :-1])
    out[:, :-1] = op(out[:, :-1], rows[:, 1:])
    return out


def sharpen_coast(heights: np.ndarray, water: np.ndarray, half: float, cliffs: dict,
                  coast_sea: np.ndarray | None = None) -> np.ndarray:
    """GLO-30 heights with cliff coasts made sheer. GLO-30 makes a cliff a 30 m slope into the sea.
    The sea beside land stands at sea level, so the coast drops where WorldCover puts the water's
    edge. Land within the region's reach_m of it takes the height found reach_m straight inland,
    where that is min_m high, fading to its own height at the reach. A cliff has a shoulder: another
    reach inland the ground rises little. There the edge rises up to raise_m; where the ground goes
    on climbing it is a hillside, and the edge rises a fifth of that at most. OSM's coastline, where
    there is one, says where the sea is to the texel, gullies the DEM averages away included."""
    sea = coast_sea.copy() if coast_sea is not None else water & (heights <= SEA_LEVEL_M)
    if not sea.any():
        return heights
    size = heights.shape[0]
    px = 2 * half / size
    # GLO-30 smears a cliff over the sea beside it too: water joined to the sea within reach of
    # the shore is sea, however high the DEM puts it. Twice the cliff reach, so a lough behind a
    # cliff top keeps its level.
    for _ in range(0 if coast_sea is not None else max(1, round(cliffs["reach_m"] * 2 / px))):
        sea |= grow(sea, np.logical_or) & water
    steps = max(1, round(cliffs["reach_m"] / px))
    # Steps from the sea, a little beyond the reach, so "inland" has a direction there too.
    distance = np.full(heights.shape, 2.0 * steps + 1)
    distance[sea] = 0
    near = sea
    for step in range(1, 2 * steps + 1):
        grown = grow(near, np.logical_or)
        distance[grown & ~near] = step
        near = grown
    gy, gx = np.gradient(box_mean(distance, 1))
    norm = np.maximum(np.hypot(gx, gy), 1e-6)
    rows, cols = np.mgrid[0:size, 0:size].astype(np.float32)

    def inland(reach: float) -> np.ndarray:
        """Bilinear height `reach` texels inland of each texel."""
        y = np.clip(rows + gy / norm * reach, 0, size - 1.001)
        x = np.clip(cols + gx / norm * reach, 0, size - 1.001)
        y0, x0 = y.astype(int), x.astype(int)
        fy, fx = y - y0, x - x0
        return ((heights[y0, x0] * (1 - fx) + heights[y0, x0 + 1] * fx) * (1 - fy)
                + (heights[y0 + 1, x0] * (1 - fx) + heights[y0 + 1, x0 + 1] * fx) * fy)

    top, beyond = inland(steps), inland(2 * steps)
    shoulder = beyond - top < 0.5 * (top - heights)
    fade = np.clip(1 - (distance - 1) / steps, 0, 1)
    raise_m = np.where(shoulder, cliffs["raise_m"], cliffs["raise_m"] / 5) * fade
    rim = ~water & (distance <= steps) & (top >= cliffs["min_m"])
    lift = np.where(rim, np.maximum(np.minimum(top, heights + raise_m) - heights, 0.0), 0.0)
    # Where the coast turns sharply, the inland direction and the shoulder test jump from texel to
    # texel; the lift is averaged over the land around, so the rim rises as a shelf, not spikes.
    land = (~water).astype(np.float32)
    r = max(1, round(RIM_SMOOTH_M / px))
    out = heights + np.where(water, 0.0, box_mean(lift, r) / np.maximum(box_mean(land, r), 1e-6))
    # Flat at sea level, and the shore no lower: cubic upsampling overshoots below a cliff, and the
    # wall would hang into the hollow.
    out[sea] = 0.0
    shore = distance <= steps
    out[shore] = np.maximum(out[shore], 0.0)
    return out


def sharpen_relief(heights: np.ndarray, half: float, rule: dict) -> np.ndarray:
    """GLO-30 heights with the relief finer than its 30 m cells brought back by an unsharp mask:
    ridges stand sharper and gullies cut deeper, by the region's amount of the difference from the
    mean over radius_m. Only where a texel is finer than the radius."""
    px = 2 * half / heights.shape[0]
    r = round(rule["radius_m"] / px)
    if r < 3 or rule["amount"] <= 0:
        return heights
    return heights + rule["amount"] * (heights - box_mean(heights, r))


def sharpen_summits(heights: np.ndarray, summits: list, half: float, rule: dict) -> np.ndarray:
    """GLO-30 heights with mapped summits restored: GLO-30 rounds a peak down into a dome as wide as
    its 30 m cells, which hides the drop from anyone standing on top. Within the region's reach_m of
    a peak with a surveyed height, the top rises to that height and the ground falls away as a cone
    toward the height found at the reach in each direction, never more than max_cut_m below GLO-30."""
    size = heights.shape[0]
    px = 2 * half / size
    reach = rule["reach_m"]
    out = heights.copy()
    for x, y, ele in summits:
        if max(abs(x), abs(y)) >= half - reach:
            continue
        i0, i1 = max(int((x - reach + half) / px), 0), min(int((x + reach + half) / px) + 1, size)
        j0, j1 = max(int((half - y - reach) / px), 0), min(int((half - y + reach) / px) + 1, size)
        cols = (np.arange(i0, i1) + 0.5) * px - half
        rows = half - (np.arange(j0, j1) + 0.5) * px
        dx, dy = np.meshgrid(cols - x, rows - y)
        d = np.hypot(dx, dy)
        window = out[j0:j1, i0:i1]
        # Only peaks that stand above their surroundings, not a name on a slope.
        if ele < window.max() - 15:
            continue
        # The ground at the reach straight out from the peak, bilinear.
        u = (x + dx / np.maximum(d, 1e-6) * reach + half) / px - 0.5
        v = (half - (y + dy / np.maximum(d, 1e-6) * reach)) / px - 0.5
        ui, vi = np.clip(u.astype(int), 0, size - 2), np.clip(v.astype(int), 0, size - 2)
        fu, fv = np.clip(u - ui, 0, 1), np.clip(v - vi, 0, 1)
        rim = ((heights[vi, ui] * (1 - fu) + heights[vi, ui + 1] * fu) * (1 - fv)
               + (heights[vi + 1, ui] * (1 - fu) + heights[vi + 1, ui + 1] * fu) * fv)
        t = np.clip(d / reach, 0, 1)
        cone = ele - t * (ele - rim)
        shaped = np.maximum(cone, window - rule["max_cut_m"])
        out[j0:j1, i0:i1] = np.where(d < reach, shaped, window)
    return out


def wms_map(service: str, layer: str, fmt: str, bbox, px: int, path: Path, info: tuple[int, int] | None = None) -> Path:
    """One WMS 1.3.0 map of a WGS84 box (latitude first, as 1.3.0 states EPSG:4326), or the
    feature info at a pixel of that map."""
    west, south, east, north = bbox
    params = {"SERVICE": "WMS", "VERSION": "1.3.0", "REQUEST": "GetMap", "LAYERS": layer, "STYLES": "",
              "CRS": "EPSG:4326", "BBOX": f"{south},{west},{north},{east}", "WIDTH": px, "HEIGHT": px, "FORMAT": fmt}
    if info:
        params.update(REQUEST="GetFeatureInfo", QUERY_LAYERS=layer, I=info[0], J=info[1], INFO_FORMAT="text/xml")
    return http_download(f"{service}?{urllib.parse.urlencode(params)}", path)


def onto_level(bands: np.ndarray, bounds, crs: str, k: int, frame: str, resampling: str, work: Path) -> np.ndarray:
    """uint8 bands over a box in `crs`, warped onto level k's grid."""
    half = HALF0_M * 2**k
    src, dst = work / "map.tif", work / "map-warp.tif"
    profile = {"driver": "GTiff", "height": bands.shape[1], "width": bands.shape[2], "count": bands.shape[0],
               "dtype": "uint8", "crs": crs, "transform": rasterio.transform.from_bounds(*bounds, bands.shape[2], bands.shape[1])}
    with rasterio.open(src, "w", **profile) as f:
        f.write(bands)
    # A fourth band is data here, not alpha.
    subprocess.run(["gdalwarp", "-q", "-overwrite", "-nosrcalpha", "-t_srs", frame, "-te", str(-half), str(-half), str(half),
                    str(half), "-ts", str(SIZE), str(SIZE), "-r", resampling, str(src), str(dst)], check=True)
    with rasterio.open(dst) as f:
        return f.read()


def lithology(text: str) -> int:
    """The lithology that a unit's description names: its earliest rock word, variegated where
    the variegated word follows it or names the unit alone."""
    lower = text.lower()
    found = [(lower.find(word), word, name) for name, words in ROCK_WORDS.items() for word in words if word in lower]
    at, word, name = min(found) if found else (-1, "", "unknown")
    tint = lower.find(VARIEGATED)
    if tint >= 0 and (not found or 0 <= tint - at - len(word) <= VARIEGATED_AFTER):
        return LITHOLOGY["variegated-marl"]
    return LITHOLOGY[name]


def igme_level(k: int, frame: str, bbox, work: Path, units: dict) -> np.ndarray:
    """R8 on level k: the lithology of the geological unit, 0 where its description names none.
    The map's fill colours stand for units; hatching and antialiased edges are minor colours, which
    take the nearest major one. `units` caches each fill colour's description across levels."""
    service, layer, _ = GEOLOGY["igme"]
    px = 2048
    with rasterio.open(wms_map(service, layer, "image/png", bbox, px, work / f"geology-{k}.png")) as f:
        rgb = f.read()[:3].astype(np.int64)
    code = (rgb[0] << 16) | (rgb[1] << 8) | rgb[2]
    values, counts = np.unique(code, return_counts=True)
    # Black is the map's boundary lines and labels.
    majors = values[(counts >= 0.002 * code.size) & (values != 0)]
    label = np.full(code.shape, -1)
    for i, value in enumerate(majors):
        label[code == value] = i
    for _ in range(16):
        for axis, step in ((0, 1), (0, -1), (1, 1), (1, -1)):
            shifted = np.roll(label, step, axis=axis)
            fill = (label < 0) & (shifted >= 0)
            label[fill] = shifted[fill]
    label[label < 0] = int(np.argmax([np.sum(label == i) for i in range(len(majors))]))
    lithologies = np.zeros(len(majors), np.uint8)
    for i, value in enumerate(majors):
        if value not in units:
            inside = label == i
            for axis, step in ((0, 4), (0, -4), (1, 4), (1, -4)):
                inside &= np.roll(label == i, step, axis=axis)
            js, is_ = np.nonzero(inside if inside.any() else label == i)
            j, x = js[len(js) // 2], is_[len(is_) // 2]
            body = wms_map(service, layer, "image/png", bbox, px, work / f"info-{k}-{value}.xml", info=(int(x), int(j))).read_text()
            match = re.search(r'DescripciónUnidadGeológica="([^"]*)"', body)
            units[value] = match.group(1) if match else ""
        lithologies[i] = lithology(units[value])
    return onto_level(lithologies[label][None].astype(np.uint8), bbox, "EPSG:4326", k, frame, "near", work)[0]


def box_mean(a: np.ndarray, r: int) -> np.ndarray:
    """The mean over a (2r + 1)² box, edges repeated."""
    n = 2 * r + 1
    s = np.pad(np.pad(a, r, mode="edge").cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    return (s[n:, n:] - s[:-n, n:] - s[n:, :-n] + s[:-n, :-n]) / (n * n)


def canopy_of(glo30: np.ndarray, dtm: np.ndarray, material: np.ndarray, half: float) -> np.ndarray:
    """GLO-30, a surface model, minus the DTM on woodland, clamped to 0–60 m. The DTM is averaged
    over GLO-30's own 30 m first, so terrain detail does not read as canopy."""
    canopy = np.clip(glo30 - box_mean(dtm, round(15 / (2 * half / SIZE))), 0, 60)
    canopy[~WOODLAND[material]] = 0
    return canopy


def sample(level: np.ndarray, half: float, x, y):
    """Nearest texel of a level at metres east and north."""
    size = level.shape[0]
    px = 2 * half / size
    i = np.clip(((np.asarray(x) + half) / px).astype(int), 0, size - 1)
    j = np.clip(((half - np.asarray(y)) / px).astype(int), 0, size - 1)
    return level[j, i]


def scatter_trees(material0, roads0, water0, canopy0, regions0, leaves: np.ndarray) -> np.ndarray:
    """float32 x, y, height, type (0 conifer, 1 broadleaf) for every tree within TREE_RADIUS_M:
    jittered on woodland, outside the road buffers and water, as tall as the canopy says. A forest
    of unknown leaf type takes its region's `leaf` (`leaves`, a forest per region). GLO-30 alone
    cannot measure a canopy, so without a DTM trees stand NO_CANOPY_M tall."""
    rng = np.random.default_rng(1)
    grid = np.arange(-TREE_RADIUS_M, TREE_RADIUS_M, TREE_SPACING_M) + TREE_SPACING_M / 2
    x, y = (axis.ravel() + rng.uniform(-0.4, 0.4, axis.size) * TREE_SPACING_M for axis in np.meshgrid(grid, grid))
    half = HALF0_M
    edge = sample(roads0, half, x, y)[:, 0] / 10 - 5
    canopy = sample(canopy0, half, x, y) if canopy0 is not None else rng.uniform(*NO_CANOPY_M, x.size)
    keep = ((np.hypot(x, y) <= TREE_RADIUS_M) & WOODLAND[sample(material0, half, x, y)]
            & (edge > 1.5) & (sample(water0, half, x, y) < 64) & (canopy > 3))
    x, y, canopy = x[keep], y[keep], canopy[keep]
    forest = sample(material0, half, x, y)
    forest = np.where(forest == MATERIAL["forest"], leaves[sample(regions0, half, x, y)], forest)
    broad = np.where(forest == MATERIAL["mixed-forest"], rng.random(forest.size) < 0.5, forest == MATERIAL["broadleaf-forest"])
    height = np.clip(canopy, NO_CANOPY_M[0], 45) * rng.uniform(0.8, 1.15, canopy.size)
    return np.stack([x, y, height, broad.astype(np.float32)], axis=-1).astype(np.float32)


def nlcm(spot_id: str, lat: float, lon: float, to_wgs84) -> list[tuple[list, str]]:
    """The land cover map's polygons over the national levels as WGS84 rings with their material,
    cached per position. The service pages 2000 polygons at a time, slowly, so pages run three
    at once."""
    cached = SOURCES / "nlcm" / f"{spot_id}_{lat:.5f}_{lon:.5f}.json"
    if not cached.exists():
        with machine_lock("landcover-nlcm"):
            half = HALF0_M * 2 ** (NATIONAL_LEVELS - 1)
            w, s, e, n = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
            query = {"where": "1=1", "geometry": f"{w},{s},{e},{n}", "geometryType": "esriGeometryEnvelope", "inSR": 4326,
                     "spatialRel": "esriSpatialRelIntersects", "outFields": "LEVEL_2_VALUE", "outSR": 4326, "f": "json"}
            get = lambda **extra: json.loads(urllib.request.urlopen(
                f"{NLCM[0]}/query?{urllib.parse.urlencode({**query, **extra})}", timeout=600).read())
            count = get(returnCountOnly="true", returnGeometry="false")["count"]
            with ThreadPoolExecutor(3) as pool:
                pages = pool.map(lambda offset: get(returnGeometry="true", resultOffset=offset, resultRecordCount=2000),
                                 range(0, count, 2000))
                rows = [[f["geometry"]["rings"], f["attributes"]["LEVEL_2_VALUE"]] for page in pages for f in page["features"]
                        if f.get("geometry")]
            write_atomic(cached, json.dumps(rows))
    material = lambda value: next((m for word, m in NLCM_WORDS if word in (value or "").lower()), "none")
    return [(rings, material(value)) for rings, value in json.loads(cached.read_text())]


def habmos(spot_id: str, lat: float, lon: float, to_wgs84) -> list[tuple[list, str]]:
    """HabMoS polygons over the national levels as WGS84 rings with their material, cached like
    `nlcm`. One request: the service returns up to a million features."""
    cached = SOURCES / "habmos" / f"{spot_id}_{lat:.5f}_{lon:.5f}.json"
    if not cached.exists():
        with machine_lock("landcover-habmos"):
            half = HALF0_M * 2 ** (NATIONAL_LEVELS - 1)
            w, s, e, n = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
            query = {"service": "WFS", "version": "2.0.0", "request": "GetFeature", "typeNames": HABMOS[1],
                     "outputFormat": "application/json", "srsName": "EPSG:4326",
                     "bbox": f"{s},{w},{n},{e},urn:ogc:def:crs:EPSG::4326"}
            request = urllib.request.Request(f"{HABMOS[0]}?{urllib.parse.urlencode(query)}", headers={"User-Agent": USER_AGENT})
            features = json.loads(urllib.request.urlopen(request, timeout=600).read())["features"]
            rows = []
            for f in features:
                geometry = f.get("geometry") or {}
                polygons = [geometry["coordinates"]] if geometry.get("type") == "Polygon" else geometry.get("coordinates", [])
                rows += [[rings, f["properties"].get("HABITAT_CODE")] for rings in polygons]
            write_atomic(cached, json.dumps(rows))

    def material(code: str | None) -> str:
        matches = [prefix for prefix in HABMOS_CODES if (code or "").startswith(prefix)]
        return HABMOS_CODES[max(matches, key=len)] if matches else "none"

    return [(rings, material(code)) for rings, code in json.loads(cached.read_text())]


# Land cover maps by a spot's `landcover` key: the polygons, and their credit.
LANDCOVER = {"nlcm": (nlcm, NLCM[1]), "habmos": (habmos, HABMOS[2])}


def orchard_trees(orchards: list[np.ndarray]) -> np.ndarray:
    """float32 x, y, height, type 2 for orchard trees within TREE_RADIUS_M, in rows along each
    orchard's longest edge."""
    rng = np.random.default_rng(2)
    rows = []
    for ring in orchards:
        if np.hypot(*ring.T).min() > TREE_RADIUS_M:
            continue
        edges = np.diff(ring, axis=0)
        u = edges[np.argmax(np.hypot(*edges.T))]
        u = u / np.hypot(*u)
        v = np.array([-u[1], u[0]])
        a, b = ring @ u, ring @ v
        grid_u, grid_v = np.meshgrid(np.arange(a.min(), a.max(), ORCHARD_SPACING_M), np.arange(b.min(), b.max(), ORCHARD_SPACING_M))
        points = np.outer(grid_u.ravel(), u) + np.outer(grid_v.ravel(), v)
        inside = inside_ring(points, ring) & (np.hypot(*points.T) <= TREE_RADIUS_M)
        for x, y in points[inside]:
            rows.append([x, y, rng.uniform(3.5, 5.0), 2.0])
    return np.array(rows, np.float32).reshape(-1, 4)


def inside_ring(points: np.ndarray, ring: np.ndarray) -> np.ndarray:
    """Even-odd rule for each point against a closed ring."""
    x, y = points[:, 0][:, None], points[:, 1][:, None]
    ax, ay, bx, by = ring[:-1, 0], ring[:-1, 1], ring[1:, 0], ring[1:, 1]
    crosses = ((ay > y) != (by > y)) & (x < ax + (y - ay) * (bx - ax) / np.where(by == ay, 1e-9, by - ay))
    return crosses.sum(axis=1) % 2 == 1


def densify(xy: np.ndarray, step: float) -> np.ndarray:
    """The line with extra vertices so no segment is longer than `step`."""
    out = [xy[0]]
    for a, b in zip(xy[:-1], xy[1:]):
        n = max(1, int(np.ceil(np.hypot(*(b - a)) / step)))
        out += [a + (b - a) * (i / n) for i in range(1, n + 1)]
    return np.array(out)


def wall_pieces(walls: list[tuple[np.ndarray, int]]) -> np.ndarray:
    """float32 x0, y0, x1, y1, kind (0 wall, 1 hedge, 2 crest) for pieces within WALL_HALF_M."""
    rows = []
    for xy, kind in walls:
        xy = densify(xy, WALL_PIECE_M)
        for a, b in zip(xy[:-1], xy[1:]):
            if max(np.abs(a).max(), np.abs(b).max()) <= WALL_HALF_M:
                rows.append([*a, *b, kind])
    return np.array(rows, np.float32).reshape(-1, 5)


def pack(array: np.ndarray) -> bytes:
    return gzip.compress(np.ascontiguousarray(array).tobytes(), compresslevel=6)


def encode(heights: np.ndarray) -> bytes:
    """uint16 = (h + 500 m) · 10, little-endian, gzip."""
    return pack(np.clip(np.round((heights + 500.0) * 10.0), 0, 65535).astype("<u2"))


def answered(query: str, text: str) -> str:
    """The text, unless it is a failed query's: that still answers, as JSON with an error remark
    or as CSV with nothing at all."""
    remark = json.loads(text).get("remark", "") if query.startswith("[out:json]") else "" if text.strip() else "error: no rows"
    if "error" in remark:
        raise http.client.HTTPException(remark)
    return text


def overpass(query: str) -> str:
    """Overpass's answer: from the local Overpass where there is one, else from each public
    instance in turn while they are busy. Busy spells last many minutes, so the tries span about
    half an hour."""
    if LOCAL_OVERPASS.exists():
        return answered(query, subprocess.run([str(LOCAL_OVERPASS / "bin/osm3s_query"), f"--db-dir={LOCAL_OVERPASS / 'db'}"],
                                              input=query, capture_output=True, text=True, check=True).stdout)
    tries = 8
    for attempt in range(tries):
        request = urllib.request.Request(OVERPASS[attempt % len(OVERPASS)], headers={"User-Agent": USER_AGENT},
                                         data=urllib.parse.urlencode({"data": query}).encode())
        try:
            with urllib.request.urlopen(request, timeout=900) as response:
                return answered(query, response.read().decode())
        except (OSError, http.client.HTTPException) as error:
            # Busy is 429 or 500 to 504, a dropped connection, or no answer at all.
            if isinstance(error, urllib.error.HTTPError) and error.code not in (429, 500, 502, 503, 504) or attempt == tries - 1:
                raise
        time.sleep(60 * (attempt + 1))


def cached_overpass(path: Path, query: str) -> str:
    """Overpass's answer, cached at `path`, because a bake is rerun far more often than OSM changes.
    One query at a time on this machine, as the public instances ration queries per address; a bake
    that waited reads the answer the other one fetched."""
    if not path.exists():
        with machine_lock("overpass"):
            if not path.exists():
                write_atomic(path, overpass(query))
    return path.read_text(encoding="utf-8")


def osm(spot_id: str, lat: float, lon: float, layer: str, statements: str) -> list[dict]:
    """The ways and relations, with their geometry, that the statements find, cached per position."""
    text = cached_overpass(SOURCES / "osm" / f"{spot_id}_{lat:.5f}_{lon:.5f}{layer}.json",
                           f"[out:json][timeout:600];({statements});out geom;")
    return [element for element in json.loads(text)["elements"] if element.get("geometry") or element.get("members")]


def overpass_box(to_wgs84, half: float) -> str:
    """±half metres around the eye as an Overpass box: south, west, north, east."""
    w, s, e, n = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
    return f"{s},{w},{n},{e}"


def osm_summits(to_wgs84) -> list[tuple[str, ...]]:
    """Named peaks and volcanoes with a height tag over the last level, as lat, lon, ele, from the
    summit tiles it touches. The header row keeps a tile without summits from reading as a failed
    query. A summit on a tile edge is in both tiles, and is taken once."""
    half = HALF0_M * 2 ** (LEVELS - 1)
    west, south, east, north = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
    t = SUMMIT_TILE_DEG
    rows = {}
    for lat in range(math.floor(south / t) * t, math.floor(north / t) * t + 1, t):
        for lon in range(math.floor(west / t) * t, math.floor(east / t) * t + 1, t):
            text = cached_overpass(SOURCES / "osm" / "summits" / f"{lat}_{lon}.csv",
                                   f'[out:csv(::lat,::lon,ele;true)][timeout:600];'
                                   f'node["natural"~"^(peak|volcano)$"]["name"]["ele"]({lat},{lon},{lat + t},{lon + t});out;')
            rows.update(dict.fromkeys(tuple(row) for row in (line.split("\t") for line in text.splitlines()[1:]) if len(row) == 3))
    return list(rows)


def osm_ways(spot_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Highways and forests with a leaf type over the road levels, buildings over BUILDING_HALF_M."""
    box, near = overpass_box(to_wgs84, HALF0_M * 2 ** (ROAD_LEVELS - 1)), overpass_box(to_wgs84, BUILDING_HALF_M)
    return osm(spot_id, lat, lon, "", f'way["highway"]({box});way["landuse"="forest"]["leaf_type"]({box});'
               f'way["natural"="wood"]["leaf_type"]({box});way["building"]({near});')


def osm_water(spot_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Waterways and water areas over the water levels. Lakes and wide rivers mapped as
    multipolygon relations come from `osm_water_areas`."""
    box = overpass_box(to_wgs84, HALF0_M * 2 ** (WATER_LEVELS - 1))
    return osm(spot_id, lat, lon, "_water", f'way["waterway"~"^(river|stream|canal|riverbank)$"]({box});'
               f'way["natural"="water"]({box});way["landuse"="reservoir"]({box});')


def osm_water_areas(spot_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Lakes, reservoirs and wide rivers mapped as multipolygon relations over the water levels,
    each with its member ways' roles and geometry."""
    box = overpass_box(to_wgs84, HALF0_M * 2 ** (WATER_LEVELS - 1))
    return osm(spot_id, lat, lon, "_waterareas", f'relation["natural"="water"]({box});'
               f'relation["waterway"="riverbank"]({box});relation["landuse"="reservoir"]({box});')


def close_rings(lines: list[np.ndarray]) -> list[np.ndarray]:
    """A multipolygon's member ways joined end to end into closed rings; a ring the query box cut
    open is dropped."""
    lines, rings = list(lines), []
    while lines:
        ring = lines.pop()
        while np.hypot(*(ring[0] - ring[-1])) > 0.5:
            following = next((i for i, line in enumerate(lines) if np.hypot(*(ring[-1] - line[0])) <= 0.5
                              or np.hypot(*(ring[-1] - line[-1])) <= 0.5), None)
            if following is None:
                break
            line = lines.pop(following)
            ring = np.concatenate([ring, (line if np.hypot(*(ring[-1] - line[0])) <= 0.5 else line[::-1])[1:]])
        if len(ring) >= 4 and np.hypot(*(ring[0] - ring[-1])) <= 0.5:
            rings.append(ring)
    return rings


def osm_coastline(spot_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """OSM's coastline over the water levels: lines with the land on their left, mapped from imagery
    to a few metres, gullies and stacks included."""
    box = overpass_box(to_wgs84, HALF0_M * 2 ** (WATER_LEVELS - 1))
    return osm(spot_id, lat, lon, "_coast", f'way["natural"="coastline"]({box});')


def osm_features(spot_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Walls, hedges, orchards, aretes and ridges over the road levels."""
    box = overpass_box(to_wgs84, HALF0_M * 2 ** (ROAD_LEVELS - 1))
    return osm(spot_id, lat, lon, "_features", f'way["barrier"~"^(wall|retaining_wall|hedge)$"]({box});'
               f'way["natural"~"^(arete|ridge)$"]({box});way["landuse"="orchard"]({box});')


def osm_areas(spot_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Natural and landuse areas that may name a material, ways and multipolygons, over the water
    levels."""
    box = overpass_box(to_wgs84, HALF0_M * 2 ** (WATER_LEVELS - 1))
    values = {"natural": "|".join([*OSM_NATURAL, "wetland"]), "landuse": "|".join(OSM_LANDUSE)}
    return osm(spot_id, lat, lon, "_areas", "".join(f'{kind}["{key}"~"^({v})$"]({box});' for kind in ("way", "relation")
                                                   for key, v in values.items()))


def area_material(tags: dict) -> str | None:
    if tags.get("natural") == "wetland":
        return WETLANDS.get(tags.get("wetland"))
    return OSM_NATURAL.get(tags.get("natural")) or OSM_LANDUSE.get(tags.get("landuse"))


# Lifts the page draws, with cables and pylons, out to ±10 km where a gondola still shows.
AERIALWAYS = ("cable_car", "gondola", "mixed_lift", "chair_lift", "drag_lift", "t-bar", "j-bar", "platter")
AERIALWAY_HALF_M = 10000.0


def osm_aerialways(spot_id: str, lat: float, lon: float, to_wgs84) -> list[dict]:
    """Cable cars, gondolas and ski lifts over AERIALWAY_HALF_M."""
    box = overpass_box(to_wgs84, AERIALWAY_HALF_M)
    return osm(spot_id, lat, lon, "_aerialways", f'way["aerialway"~"^({"|".join(AERIALWAYS)})$"]({box});')


def landmark(tags: dict) -> str:
    """The building value, or the landmark it is: a lighthouse, or a ruined castle or tower."""
    if tags.get("man_made") == "lighthouse":
        return "lighthouse"
    if tags.get("ruins") == "yes" or tags.get("historic") in ("castle", "tower", "ruins") or tags["building"] == "ruins":
        return "ruin"
    return tags["building"]


def elevation_tag(text: str) -> float | None:
    match = re.match(r"\s*(-?\d+(?:[.,]\d+)?)", text.replace("'", ""))
    return float(match.group(1).replace(",", ".")) if match else None


def places_at(places: list[dict], lons: np.ndarray, lats: np.ndarray) -> list[str | None]:
    """The lower-case ISO code of the place that holds each point, None where none does. Only the
    places near the points' box are tested."""
    found = [None] * len(lons)
    if not len(lons):
        return found
    w, s, e, n = lons.min() - 0.5, lats.min() - 0.5, lons.max() + 0.5, lats.max() + 0.5
    for place in places:
        (x0, y0), (x1, y1) = np.array([pair for ring in place["rings"] for pair in zip(ring, ring[1:])]).transpose(1, 2, 0)
        if x0.max() < w or x0.min() > e or y0.max() < s or y0.min() > n:
            continue
        for at in range(0, len(lons), 1000):
            lon, lat = lons[at:at + 1000, None], lats[at:at + 1000, None]
            with np.errstate(divide="ignore", invalid="ignore"):
                crossings = (((y0 > lat) != (y1 > lat)) & (lon < x0 + (lat - y0) * (x1 - x0) / (y1 - y0))).sum(axis=1)
            for i in np.flatnonzero(crossings % 2):
                found[at + i] = place["iso"]
    return found


def bake(spot: dict, colour: bool, snow: bool, dtm_levels: int, baseline: bool) -> None:
    tools = ("gdalwarp", "gdal_translate", "gdalbuildvrt", "ogr2ogr", PROXIMITY)
    if missing := [tool for tool in tools if not shutil.which(tool)]:
        sys.exit(f"bake.py needs the GDAL command-line tools on PATH (Ubuntu: gdal-bin, python3-gdal); not found: {', '.join(missing)}")
    out = CACHE / spot["id"]
    lat, lon = spot["lat"], spot["lon"]
    table = region_table()
    chain = region_of(table, spot["file"], lat, lon)
    places = json.loads(COUNTRIES.read_text())
    country, subdivision = place_at(places["countries"], lat, lon), place_at(places["subdivisions"], lat, lon)
    tz = TimezoneFinder().timezone_at(lng=lon, lat=lat)
    dtm = spot.get("dtm")
    # A typo would otherwise fail only after minutes of Overpass and warping.
    for key, known in (("dtm", {*DTM_SOURCES, *BOXED, "scot"}), ("landcover", LANDCOVER), ("geology", GEOLOGY)):
        if spot.get(key) and spot[key] not in known:
            sys.exit(f"{spot['id']}: unknown {key} {spot[key]!r}; known: {', '.join(sorted(known))}")
    print(f"  region {chain[-1]}, country {country}, subdivision {subdivision}, {tz}", flush=True)
    # Colour and snow take most of a bake's time, so a rebake keeps them while the spot stays where it was.
    s2 = {(k, q): out / f"s2-q{q}-{k}.bin.gz" for k in range(S2_LEVELS) for q in range(1, 5)}
    snows = {k: out / f"snow-{k}.bin.gz" for k in range(SNOW_FIRST, LEVELS)}
    before = json.loads((out / "meta.json").read_text()) if (out / "meta.json").exists() else {}
    moved = (before.get("lat"), before.get("lon")) != (lat, lon)
    colour = colour or moved or not all(path.exists() for path in s2.values())
    seasons = (before.get("snow_first_season"), before.get("snow_seasons")) != (FIRST_SEASON, len(SNOW_SEASONS))
    snow = snow or moved or seasons or not all(path.exists() for path in snows.values())
    if colour or snow:
        cdse_credentials()
    out.mkdir(parents=True, exist_ok=True)
    frame = f"+proj=aeqd +lat_0={lat} +lon_0={lon} +datum=WGS84 +units=m +no_defs"
    to_wgs84 = Transformer.from_crs(CRS.from_proj4(frame), "EPSG:4326", always_xy=True)
    sizes = []
    to_frame = Transformer.from_crs("EPSG:4326", CRS.from_proj4(frame), always_xy=True)

    def metres(way: dict) -> np.ndarray:
        return np.array(to_frame.transform([n["lon"] for n in way["geometry"]], [n["lat"] for n in way["geometry"]])).T

    raw = osm_summits(to_wgs84)
    xs, ys = to_frame.transform([float(r[1]) for r in raw], [float(r[0]) for r in raw])
    summits = [(round(x), round(y), ele) for x, y, (_, _, text) in zip(xs, ys, raw)
               if (ele := elevation_tag(text)) is not None]

    ways = osm_ways(spot["id"], lat, lon, to_wgs84)
    codes = {name: i + 1 for i, name in enumerate(ROAD_WIDTHS)}
    roads = [(metres(w), codes[surface], road_width(w["tags"], surface), w["tags"].get("bridge", "no") not in GROUND_BRIDGES)
             for w in ways if "highway" in w["tags"] and (surface := road_surface(w["tags"]))]
    # Bridges near the eye are drawn as decks. The road field keeps their footprint, unpainted
    # (code 0), so trees and plants stay off them; farther bridges are painted as before.
    decked = [bridge and passes_within(xy, BUILDING_HALF_M) for xy, _, _, bridge in roads]
    decks = chain_bridges([(xy, code, width) for (xy, code, width, _), deck in zip(roads, decked) if deck])
    roads = [(xy, 0 if deck else code, width, bridge) for (xy, code, width, bridge), deck in zip(roads, decked)]
    forests = [([metres(w)], MATERIAL[LEAF[w["tags"]["leaf_type"]]]) for w in ways
               if w["tags"].get("leaf_type") in LEAF and w["geometry"][0] == w["geometry"][-1]]
    print(f"  OSM: {len(roads)} roads, {len(forests)} forests with a leaf type, "
          f"{sum('building' in w['tags'] for w in ways)} buildings", flush=True)
    rivers, streams, lakes = [], [], []
    for way in osm_water(spot["id"], lat, lon, to_wgs84):
        tags, xy = way["tags"], metres(way)
        closed = way["geometry"][0] == way["geometry"][-1]
        # A rambla or a seasonal pond is a dry bed most of the year.
        if tags.get("intermittent") == "yes" or tags.get("seasonal") == "yes":
            continue
        if tags.get("waterway") in WATERWAY_WIDTHS and tags.get("tunnel") is None:
            width = elevation_tag(tags.get("width", "")) or WATERWAY_WIDTHS[tags["waterway"]]
            (streams if tags["waterway"] == "stream" else rivers).append((xy, width / 2))
        elif closed and tags.get("waterway") != "stream":
            lakes.append(xy)
    islands = []
    for relation in osm_water_areas(spot["id"], lat, lon, to_wgs84):
        if relation["tags"].get("intermittent") == "yes" or relation["tags"].get("seasonal") == "yes":
            continue
        members = [m for m in relation["members"] if m.get("type") == "way" and m.get("geometry")]
        lakes += close_rings([metres(m) for m in members if m.get("role") != "inner"])
        islands += close_rings([metres(m) for m in members if m.get("role") == "inner"])
    print(f"  OSM water: {len(rivers)} rivers and canals, {len(streams)} streams, {len(lakes)} areas", flush=True)
    coast = [metres(way) for way in osm_coastline(spot["id"], lat, lon, to_wgs84)]
    print(f"  OSM coastline: {len(coast)} ways", flush=True)
    features = osm_features(spot["id"], lat, lon, to_wgs84)
    tagged = lambda key, *values: [w for w in features if w["tags"].get(key) in values]
    walls = [(metres(w), 1 if w["tags"]["barrier"] == "hedge" else 0) for w in tagged("barrier", "wall", "retaining_wall", "hedge")]
    # Crests: the DEM rounds a knife-edge ridge off, so the page stands crags along it.
    walls += [(metres(w), 2) for w in tagged("natural", "arete", "ridge")]
    orchards = [metres(w) for w in tagged("landuse", "orchard") if w["geometry"][0] == w["geometry"][-1]]
    print(f"  OSM features: {len(walls)} walls and hedges, "
          f"{len(orchards)} orchards", flush=True)
    areas = []
    for element in osm_areas(spot["id"], lat, lon, to_wgs84):
        if not (name := area_material(element["tags"])):
            continue
        if element["type"] == "way" and element["geometry"][0] == element["geometry"][-1]:
            areas.append(([metres(element)], MATERIAL[name]))
        elif element["type"] == "relation" and (rings := close_rings([metres(m) for m in element["members"]
                                                                       if m.get("type") == "way" and m.get("geometry")])):
            areas.append((rings, MATERIAL[name]))
    print(f"  OSM areas: {len(areas)} that may name a material", flush=True)
    # Without the national maps, the spot bakes as one where none exists.
    geology = None if baseline else spot.get("geology")
    units: dict[int, str] = {}
    landcover = None if baseline else LANDCOVER.get(spot.get("landcover"))
    national = [([np.array(to_frame.transform(*np.array(ring).T)).T for ring in rings], MATERIAL[name])
                for rings, name in (landcover[0](spot["id"], lat, lon, to_wgs84) if landcover else []) if name != "none"]
    if landcover:
        print(f"  land cover: {len(national)} polygons", flush=True)
    regions, names = region_levels(table, spot["file"], to_frame, to_wgs84)
    rules = [rules_of(table, name) for name in names]
    print(f"  regions: {', '.join(names)}", flush=True)
    if names[regions[0][SIZE // 2, SIZE // 2]] != chain[-1]:
        print(f"  the eye's texel lies in {names[regions[0][SIZE // 2, SIZE // 2]]}, not in {chain[-1]}", flush=True)
    rocks = lithology_sources()
    used = set()

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        if colour:
            with machine_lock("cdse"):
                half = HALF0_M * 2 ** (S2_LEVELS - 1)
                quarters = s2_items(to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21), work)
                print(f"  Sentinel-2 mosaic tiles per quarter: {', '.join(f'Q{q} {len(v)}' for q, v in quarters.items())}")
                for k in range(S2_LEVELS):
                    for q, texels in s2_level(k, frame, quarters, work).items():
                        s2[k, q].write_bytes(pack(texels))
                    print(f"  colour level {k}", flush=True)
        if not snow:
            sizes += [(k, "snow", path.stat().st_size) for k, path in snows.items()]
        else:
            half = HALF0_M * 2 ** (LEVELS - 1)
            modis = modis_snow(to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21))
            with machine_lock("cdse"):
                for k, path in snows.items():
                    hr = hr_wsi_level(k, frame, to_wgs84, work)
                    blob = pack(np.where(hr[:, :1] == planner_snow.NO_DATA, modis_level(k, frame, modis), hr))
                    path.write_bytes(blob)
                    sizes.append((k, "snow", len(blob)))
                    print(f"  snow level {k}: HR-WSI holds {(hr[:, 0] != planner_snow.NO_DATA).mean():.0%} of the season texels", flush=True)
        grid = work / "national.tif"
        if dtm:
            half = HALF0_M * 2 ** (dtm_levels - 1)
            with machine_lock(f"dtm-{dtm}"):
                dtm_paths = dtm_rasters(dtm, to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21))
            if dtm_paths:
                warp(dtm_paths, frame, half, "cubic", overviews=False, work=work,
                     size=round(2 * half / NATIONAL_GRID_M), keep=grid)
            else:
                print(f"  no {dtm} DTM here: GLO-30 only", flush=True)
                dtm = None
        centre = slice(HEIGHT_SIZE // 2 - 1, HEIGHT_SIZE // 2 + 1)
        origin = {}
        for k in range(LEVELS):
            half = HALF0_M * 2**k
            bbox = to_wgs84.transform_bounds(-half, -half, half, half, densify_pts=21)
            glo30 = glo30_level(k, frame, bbox, work)
            if k == 0:
                origin["glo30"] = float(glo30[centre, centre].mean())
            heights = glo30
            if dtm and k < dtm_levels:
                national_heights = warp([grid], frame, half, "cubic", overviews=False, work=work, size=HEIGHT_SIZE)[0]
                gaps = national_heights == NODATA
                national_heights[gaps] = glo30[gaps]
                if k == 0:
                    origin["dtm"] = float(national_heights[centre, centre].mean())
                heights = national_heights
                blob = encode(glo30)
                (out / f"glo30-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "glo30", len(blob)))
            wet = osm_wet(rivers, lakes, islands, half) if k < WATER_LEVELS else None
            material, sea = material_level(k, frame, bbox, coarse(heights), work, coast if k < WATER_LEVELS else None, wet)
            if heights is glo30:
                region = regions[k]
                heights = by_region(heights, region, rules, half, lambda h, rule: sharpen_relief(h, half, rule["relief"]))
                at = sample(region, half, [x for x, _, _ in summits], [y for _, y, _ in summits])
                for code in np.unique(region):
                    heights = sharpen_summits(heights, [s for s, c in zip(summits, at) if c == code], half, rules[code]["summits"])
                water = fine(is_water(material))
                heights = by_region(heights, region, rules, half, lambda h, rule: sharpen_coast(
                    h, water, half, rule["cliffs"], None if sea is None else fine(sea)))
                if k == 0:
                    origin["glo30"] = float(heights[centre, centre].mean())
            if k < ROAD_LEVELS and roads:
                heights = grade_roads(heights, roads, half)
                if k == 0:
                    origin["dtm" if dtm else "glo30"] = float(heights[centre, centre].mean())
            blob = encode(heights)
            (out / f"h-{k}.bin.gz").write_bytes(blob)
            sizes.append((k, "h", len(blob)))
            # The baseline, refined in turn by each source that names its materials more precisely.
            if k < S2_LEVELS:
                quarters = snow_free({q: unpacked(s2[k, q], 4) for q in range(1, 5)})
                for q, texels in quarters.items():
                    blob = pack(texels)
                    s2[k, q].write_bytes(blob)
                    sizes.append((k, f"s2-q{q}", len(blob)))
                most = np.max([texels[..., 3] for texels in quarters.values()], axis=0)
                blob = pack(most)
                (out / f"ndvimax-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "ndvimax", len(blob)))
                material[satellite_heath(quarters, most) & np.isin(material, [MATERIAL[m] for m in MOOR_ON])] = MATERIAL["heath"]
            if k < ROAD_LEVELS:
                material = refine(material, polygons_level(forests, half))
            if (clc := clc_level(spot["id"], lat, lon, k, frame, work)).any():
                material = refine(material, clc)
                used.add(CLC[1])
            if k < WATER_LEVELS:
                material = refine(material, polygons_level(areas, half))
            if national and k < NATIONAL_LEVELS:
                material = overlay(material, polygons_level(national, half))
            rock = lithology_level(rocks, k, frame, work, used)
            if geology and k < NATIONAL_LEVELS:
                mapped = igme_level(k, frame, bbox, work, units)
                rock = np.where(mapped > 0, mapped, rock)
            rock[is_water(material)] = 0
            for name, layer in (("material", material), ("litho", rock), ("region", regions[k])):
                blob = pack(layer)
                (out / f"{name}-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, name, len(blob)))
            if k < ROAD_LEVELS:
                road = road_field(roads, half)
                blob = pack(road)
                (out / f"road-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "road", len(blob)))
            if k < WATER_LEVELS:
                water = water_field(wet, streams, material, half, work)
                blob = pack(water)
                (out / f"water-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "water", len(blob)))
            if dtm and k < dtm_levels:
                canopy = canopy_of(coarse(glo30), coarse(heights), material, half)
                blob = pack(np.round(canopy * 4).astype(np.uint8))
                (out / f"canopy-{k}.bin.gz").write_bytes(blob)
                sizes.append((k, "canopy", len(blob)))
            if k == 0:
                heights0, material0, road0, water0 = heights, material, road, water
                canopy0 = canopy if dtm else None
            if k == 1:
                heights1 = heights
            print(f"  level {k}: ±{half / 1000:g} km, {2 * half / HEIGHT_SIZE:.2f} m heights, "
                  f"{heights.min():.0f}–{heights.max():.0f} m", flush=True)

    leaves = np.array([MATERIAL[LEAF[rule["leaf"]]] for rule in rules], np.uint8)
    trees = scatter_trees(material0, road0, water0[..., 0], canopy0, regions[0], leaves)
    trees = np.concatenate([trees, orchard_trees(orchards)])
    blob = pack(trees)
    (out / "trees.bin.gz").write_bytes(blob)
    sizes.append(("-", "trees", len(blob)))
    footprints = [(metres(way), way["tags"]) for way in ways if "building" in way["tags"]]
    footprints = [(ring, tags) for ring, tags in footprints if len(ring) >= 4 and np.abs(ring).max() <= BUILDING_HALF_M]
    lons, lats = to_wgs84.transform([ring[:-1, 0].mean() for ring, _ in footprints],
                                    [ring[:-1, 1].mean() for ring, _ in footprints])
    # Each house takes the style of its own place: its subdivision, else the eye's.
    homes = places_at(places["subdivisions"], np.array(lons), np.array(lats))
    buildings = []
    for (ring, tags), home in zip(footprints, homes):
        # Level 0 holds ±1.25 km; past it a footprint takes its ground from level 1, not the edge.
        inner = np.abs(ring).max() < HALF0_M
        base = float(sample(heights0 if inner else heights1, HALF0_M * (1 if inner else 2), ring[:, 0], ring[:, 1]).min())
        buildings.append([round(base, 1), landmark(tags), elevation_tag(tags.get("building:levels", "")),
                          elevation_tag(tags.get("height", "")), [round(float(v), 1) for v in ring[:-1].ravel()],
                          home or subdivision or country])
    blob = json.dumps(buildings, separators=(",", ":")).encode()
    (out / "buildings.json").write_bytes(blob)
    sizes.append(("-", "buildings", len(blob)))

    blob = pack(wall_pieces(walls))
    (out / "walls.bin.gz").write_bytes(blob)
    sizes.append(("-", "walls", len(blob)))

    # Bridges near enough to show as decks: surface code, half-width and the line.
    bridges = [[code, round(width, 2), [round(float(v), 1) for v in xy.ravel()]] for xy, code, width in decks]
    blob = json.dumps(bridges, separators=(",", ":")).encode()
    (out / "bridges.json").write_bytes(blob)
    sizes.append(("-", "bridges", len(blob)))

    # The OSM line runs station to station; a vertex between is a pylon.
    lifts = [[way["tags"]["aerialway"], [round(float(v), 1) for v in metres(way).ravel()]]
             for way in osm_aerialways(spot["id"], lat, lon, to_wgs84)]
    blob = json.dumps(lifts, separators=(",", ":")).encode()
    (out / "aerialways.json").write_bytes(blob)
    sizes.append(("-", "aerialways", len(blob)))
    print(f"  {len(trees)} trees, {len(buildings)} buildings", flush=True)

    ground = spot.get("ground_m", origin.get("dtm", origin["glo30"]))
    print(f"  ground {ground:.1f} m; at the origin the DTM says {origin.get('dtm', float('nan')):.1f} m, "
          f"GLO-30 {origin['glo30']:.1f} m")

    credits = [CREDITS["glo30"]] + ([CREDITS.get(dtm) or DTM_SOURCES[dtm].credit(time.strftime("%Y-%m-%d"))] if dtm else [])
    snow_credit = (f"{data_registry.attribution('hr-wsi', year=time.strftime('%Y'))}; {data_registry.attribution('modis-snow')}; "
                   f"tree canopy: {data_registry.attribution('hansen-gfc')}")
    credits += [CREDITS["worldcover"], CREDITS["s2"], snow_credit, *sorted(used)]
    credits += [GEOLOGY[geology][2]] if geology else []
    credits += [landcover[1]] if national else []
    credits.append(CREDITS["osm"])

    meta = {key: spot[key] for key in ("id", "lat", "lon", "heading", "pitch") if key in spot}
    above = spot.get("eye_above_ground_m", PHONE_M)
    meta.update(ground_m=round(float(ground), 2), eye_m=round(float(ground) + above, 2), eye_above_ground_m=above,
                levels=LEVELS, size=SIZE, height_size=HEIGHT_SIZE, half0_m=HALF0_M, dtm_levels=dtm_levels if dtm else 0,
                dtm=dtm, s2_levels=S2_LEVELS, snow_first_level=SNOW_FIRST, snow_first_season=FIRST_SEASON,
                snow_seasons=len(SNOW_SEASONS), road_levels=ROAD_LEVELS, water_levels=WATER_LEVELS, shore_reach_m=SHORE_REACH_M,
                region=chain[-1], chain=chain, regions=names, country=country, subdivision=subdivision, tz=tz, credits=credits)
    if "photo_source" in spot:
        shutil.copyfile(spot["photo_source"], out / "photo.jpg")
        meta["photo"] = spot["photo"]
        credits.append(spot["photo"]["credit"])
    (out / "meta.json").write_text(json.dumps(meta, ensure_ascii=False, indent=1))

    print(f"\n{'level':>5} {'layer':>8} {'bytes':>10}")
    for row in sizes:
        print(f"{row[0]!s:>5} {row[1]:>8} {row[2]:>10,}")
    print(f"{'':>5} {'total':>8} {sum(row[2] for row in sizes):>10,}")
    season = sum(row[2] for row in sizes if row[1] in ("h", "material", "litho", "region", "road", "water", "canopy", "trees",
                                                        "buildings", "s2-q3", "ndvimax", "snow", "walls"))
    print(f"one season without the GLO-30 comparison: {season:,} bytes")
    print(f"\nwrote {out}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("spot")
    parser.add_argument("--colour", action="store_true",
                        help="read the Sentinel-2 levels again; a rebake keeps them while the spot stays where it was")
    parser.add_argument("--snow", action="store_true",
                        help="read the snow levels again; a rebake keeps them while the spot stays where it was")
    parser.add_argument("--dtm-levels", type=int, default=DTM_LEVELS,
                        help=f"levels that take the national DTM (default {DTM_LEVELS}: ±10 km)")
    parser.add_argument("--baseline", action="store_true",
                        help="leave out the spot's land cover and geology maps, as for a place without them")
    args = parser.parse_args()
    bake(load_spot(args.spot), args.colour, args.snow, args.dtm_levels, args.baseline)


if __name__ == "__main__":
    main()
