"""Germany: one row per federal state, because elevation is a state matter.

There is no national DGM service, and the states do not agree on how to publish. Three
shapes cover the six biggest: a WCS 2.0.1 (North Rhine-Westphalia, Hesse,
Baden-Württemberg), a grid of tiles at a fixed URL (Bavaria, Saxony, Thuringia), and a
STAC of cloud-optimised GeoTIFFs (Lower Saxony). All of them are keyless.

Bavaria's INSPIRE WCS answers 401 and its credentials come from the LDBV by post, so
Bavaria is a download. Saxony and Thuringia publish no WCS at all. Brandenburg's WCS
answers float32 metres but names no CRS and its heights do not agree with two published
Brandenburg summits, so it is not a row until that is understood; `README.md` says so.

The axis labels differ between the WCS states even though the grid is the same UTM zone:
NRW's coverage is `x`/`y` and the two INSPIRE coverages are `E`/`N`. A server refuses a
subset whose label it does not know, so the row carries the labels its capabilities state.
"""

from .grid import GridTiles
from .protocols import Wcs10Source, Wcs20Source
from .stac import StacSearchSource

# DHHN2016 is the German height reference, and every state below is on it. Thuringia's
# earlier 2014–2019 delivery is on DHHN92, so the adapter takes the 2020–2025 one.
DHHN = "DHHN2016"

# Saxony's and Thuringia's Quellenvermerk are the shortest form their services state, and
# neither agency publishes a full attribution sentence the way Bavaria and NRW do. The
# owner has to confirm both with GeoSN and GDI-Th before a map carrying them is published.

NW = Wcs20Source(
    "de-nw", "Germany, North Rhine-Westphalia", "DGM1 1 m", 1.0,
    "dl-de/zero-2-0", "© Geobasis NRW", DHHN, (5.8, 50.3, 9.5, 52.6),
    url="https://www.wcs.nrw.de/geobasis/wcs_nw_dgm",
    coverage="nw_dgm",
    epsg=25832,
    axes=("x", "y"),
)

HE = Wcs20Source(
    "de-he", "Germany, Hesse", "DGM1 1 m", 1.0,
    "dl-de/zero-2-0", "© HLBG Hessen", DHHN, (7.7, 49.3, 10.3, 51.7),
    url="https://inspire-hessen.de/raster/dgm1/ows",
    coverage="he_dgm1",
    epsg=25832,
    axes=("E", "N"),
)

# The Baden-Württemberg coverage answers `UInt16`, so its heights are whole metres. That
# is the archive's own quantum, so nothing is lost; the state's centimetre data is in the
# `.xyz` bulk download, which is not a raster and is not worth a second code path.
BW = Wcs20Source(
    "de-bw", "Germany, Baden-Württemberg", "DGM1 1 m (whole metres over WCS)", 1.0,
    "dl-de/by-2-0", "Datenquelle: LGL, www.lgl-bw.de, dl-de/by-2-0", DHHN,
    (7.5, 47.5, 10.5, 49.8),
    url="https://owsproxy.lgl-bw.de/owsproxy/wcs/WCS_INSP_BW_Hoehe_Coverage_DGM1",
    coverage="EL.ElevationGridCoverage",
    epsg=25832,
    axes=("E", "N"),
    scale=False,
)

BY = GridTiles(
    "de-by", "Germany, Bavaria", "DGM1 1 m", 1.0,
    "CC BY 4.0", "Bayerische Vermessungsverwaltung – www.geodaten.bayern.de", DHHN,
    (8.9, 47.2, 13.9, 50.6),
    base="https://download1.bayernwolke.de/a/dgm/dgm1/",
    name="{east}_{north}.tif",
    epsg=25832,
    tile_km=1,
)

SN = GridTiles(
    "de-sn", "Germany, Saxony", "DGM1 1 m", 1.0,
    "dl-de/by-2-0", "GeoSN", DHHN, (11.8, 50.1, 15.1, 51.7),
    base="https://geocloud.landesvermessung.sachsen.de/public.php/dav/files/JCcXyifaNdLDnxZ/",
    name="dgm1_33{east}_{north}_2_sn_tiff.zip",
    epsg=25833,
    tile_km=2,
)

TH = GridTiles(
    "de-th", "Germany, Thuringia", "DGM1 1 m", 1.0,
    "dl-de/by-2-0", "© GDI-Th, Freistaat Thüringen", DHHN, (9.8, 50.2, 12.7, 51.7),
    base="https://geoportal.geoportal-th.de/hoehendaten/DGM/dgm_2020-2025/",
    name="dgm1_32_{east}_{north}_1_th_2020-2025.zip",
    epsg=25832,
    tile_km=1,
)

MV = Wcs20Source(
    "de-mv", "Germany, Mecklenburg-Vorpommern", "DGM1 1 m", 1.0,
    "Open data, attribution required", "© GeoBasis-DE/M-V", DHHN, (10.5, 53.0, 14.5, 54.8),
    url="https://www.geodaten-mv.de/dienste/dgm_wcs",
    coverage="mv_dgm",
    epsg=25833,
    axes=("x", "y"),
)

# Saxony-Anhalt answers WCS 2.0.1 as `multipart/related`, with the TIFF a couple of
# kilobytes into the body, so this one takes the 1.0.0 request. Its coverage is named `1`.
ST = Wcs10Source(
    "de-st", "Germany, Saxony-Anhalt", "DGM1 1 m", 1.0,
    "dl-de/by-2-0", "© GeoBasis-DE / LVermGeo LSA", DHHN, (10.5, 50.9, 13.2, 53.1),
    url="https://geodatenportal.sachsen-anhalt.de/ows_INSPIRE_LVermGeo_ATKIS_EL_DGM_WCS",
    coverage="1",
    epsg=25832,
)

NI = StacSearchSource(
    "de-ni", "Germany, Lower Saxony", "DGM1 1 m", 1.0,
    "CC BY 4.0", "© LGLN", DHHN, (6.5, 51.2, 11.7, 54.0),
    search="https://dgm.stac.lgln.niedersachsen.de/search",
    asset="dgm1-tif",
)

DE = (NW, HE, BW, MV, ST, BY, SN, TH, NI)
