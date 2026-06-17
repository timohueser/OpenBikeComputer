import os
import fiona
import requests
import zipfile
from shapely.geometry import shape, box
from shapely.ops import transform
from pyproj import Transformer

CACHE_DIR = os.path.expanduser("~/.cache/obcm/land")

def get_land_polygons(pbf_bbox):
    # Ensure cache directory exists
    os.makedirs(CACHE_DIR, exist_ok=True)

    # URL for land polygons
    url = "https://osmdata.openstreetmap.de/download/land-polygons-split-3857.zip"
    zip_path = os.path.join(CACHE_DIR, "land-polygons.zip")
    shp_dir = os.path.join(CACHE_DIR, "land-polygons-split-3857")
    shp_path = os.path.join(shp_dir, "land_polygons.shp")
    version_path = os.path.join(CACHE_DIR, "land-polygons.version")

    # Check for updates if file exists
    should_download = not os.path.exists(shp_path)
    if not should_download:
        try:
            resp = requests.head(url)
            remote_version = resp.headers.get('Last-Modified')
            if os.path.exists(version_path):
                with open(version_path, 'r') as f:
                    local_version = f.read()
                if remote_version and remote_version != local_version:
                    should_download = True
            elif remote_version:
                should_download = True
        except Exception:
            # If network check fails, assume local is fine
            pass

    if should_download:
        print(f"Downloading/Updating land polygons (this might take a while)...")
        response = requests.get(url, stream=True)
        with open(zip_path, 'wb') as f:
            for chunk in response.iter_content(chunk_size=8192):
                f.write(chunk)

        print(f"Extracting...")
        with zipfile.ZipFile(zip_path, 'r') as zip_ref:
            zip_ref.extractall(CACHE_DIR)

        # Store version
        resp = requests.head(url)
        remote_version = resp.headers.get('Last-Modified')
        if remote_version:
            with open(version_path, 'w') as f:
                f.write(remote_version)

    # Clip and return polygons
    polygons = []

    # Transform PBF bbox to EPSG:3857
    transformer = Transformer.from_crs("EPSG:4326", "EPSG:3857", always_xy=True)
    min_x, min_y = transformer.transform(pbf_bbox[0], pbf_bbox[1])
    max_x, max_y = transformer.transform(pbf_bbox[2], pbf_bbox[3])
    shp_bbox = (min_x, min_y, max_x, max_y)

    bbox_poly_3857 = box(*shp_bbox)

    with fiona.open(shp_path) as src:
        # Fionas filter parameter expects coordinates in the shapefile's CRS (3857)
        for feature in src.filter(bbox=shp_bbox):
            geom = shape(feature['geometry'])
            if geom.intersects(bbox_poly_3857):
                intersection = geom.intersection(bbox_poly_3857)
                # Reproject result back to 4326 for consistency with PBF data
                back_transformer = Transformer.from_crs("EPSG:3857", "EPSG:4326", always_xy=True)
                final_poly = transform(back_transformer.transform, intersection).simplify(0.000005, preserve_topology=True)
                polygons.append(final_poly)

    return polygons
