import argparse
import sys
import os
import tempfile
import subprocess
import shapely
from tqdm import tqdm
from obcm.config import load_config
from obcm.ingest import ingest_osm
from obcm.quadtree import QuadtreeNode
from obcm.serialize import serialize_all
from obcm.land_ingest import get_land_polygons

def main():
    parser = argparse.ArgumentParser(description="OBCM Pack: OSM to OBCM Binary Converter")
    parser.add_argument("pbf", nargs='+', help="Input .osm.pbf file(s)")
    parser.add_argument("config", help="Input config.json file")
    parser.add_argument("output", help="Output .obcm file")
    parser.add_argument("--chunk-size", type=int, default=4096, help="Data chunk size (default 4096)")
    args = parser.parse_args()

    # Validate inputs
    for pbf in args.pbf:
        if not os.path.exists(pbf):
            print(f"Error: PBF file not found: {pbf}")
            sys.exit(1)
    
    if not os.path.exists(args.config):
        print(f"Error: Config file not found: {args.config}")
        sys.exit(1)

    # Handle merging if multiple files
    if len(args.pbf) > 1:
        print(f"Merging {len(args.pbf)} files...")
        temp_merged = tempfile.NamedTemporaryFile(suffix=".osm.pbf", delete=False)
        temp_merged.close()
        
        try:
            # Using subprocess to call osmium merge
            subprocess.run(["osmium", "merge", "--overwrite"] + args.pbf + ["-o", temp_merged.name], check=True)
            # Sorting is recommended after merge
            temp_sorted = tempfile.NamedTemporaryFile(suffix=".osm.pbf", delete=False)
            temp_sorted.close()
            subprocess.run(["osmium", "sort", "--overwrite", temp_merged.name, "-o", temp_sorted.name], check=True)
            pbf_to_ingest = temp_sorted.name
            
            # Clean up unsorted temp
            os.remove(temp_merged.name)
        except subprocess.CalledProcessError as e:
            print(f"Error merging/sorting files: {e}")
            os.remove(temp_merged.name)
            sys.exit(1)
    else:
        pbf_to_ingest = args.pbf[0]
        temp_sorted = None

    try:
        config = load_config(args.config)
        chunk_size = config.get("chunk_size", args.chunk_size)

        features, coastlines = ingest_osm(pbf_to_ingest, config)
    finally:
        if temp_sorted:
            os.remove(temp_sorted.name)

    if not features and not coastlines:
        print("No features found matching config. Exiting.")
        sys.exit(0)

    # Calculate global bounding box in degrees (vectorized over all geometries)
    print("Calculating bounding box...")
    all_geoms = [feat["geometry"] for feat in features] + coastlines
    min_lon, min_lat, max_lon, max_lat = shapely.total_bounds(all_geoms)

    global_bbox = (
        int(min_lon * 1e6),
        int(min_lat * 1e6),
        int(max_lon * 1e6),
        int(max_lat * 1e6)
    )

    # --- Land Generation Logic ---
    has_land_config = "natural" in config.get("features", {}) and "land" in config["features"]["natural"]
    if has_land_config:
        land_style = config["features"]["natural"]["land"]["id"]
        # Convert bbox to (min_lon, min_lat, max_lon, max_lat) in decimal degrees
        # Note: ingest_osm uses microdegrees, but land_ingest needs degrees.
        # global_bbox is in microdegrees.
        pbf_bbox_deg = (
            global_bbox[0] / 1e6,
            global_bbox[1] / 1e6,
            global_bbox[2] / 1e6,
            global_bbox[3] / 1e6
        )
        
        print("Fetching and processing land polygons...")
        land_polygons = get_land_polygons(pbf_bbox_deg)
        
        for poly in land_polygons:
            features.append({"style_id": land_style, "geometry": poly})
        
        print(f"Successfully added {len(land_polygons)} land polygons.")


    root = QuadtreeNode(global_bbox, chunk_size=chunk_size)
    for feat in tqdm(features, desc="Building Quadtree", unit="feat"):
        root.insert(feat)

    print("Serializing and writing to disk...")
    binary_data = serialize_all(root, config, global_bbox, chunk_size=chunk_size)

    with open(args.output, "wb") as f:
        f.write(binary_data)

    print("Done!")

if __name__ == "__main__":
    main()
