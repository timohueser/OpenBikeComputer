import argparse
import sys
import os
import struct
from obcm.config import load_config
from obcm.ingest import ingest_osm
from obcm.quadtree import QuadtreeNode
from obcm.serialize import serialize_all

def main():
    parser = argparse.ArgumentParser(description="OBCM Pack: OSM to OBCM Binary Converter")
    parser.add_argument("pbf", help="Input .osm.pbf file")
    parser.add_argument("config", help="Input config.json file")
    parser.add_argument("output", help="Output .obcm file")
    parser.add_argument("--chunk-size", type=int, default=4096, help="Data chunk size (default 4096)")
    args = parser.parse_args()

    if not os.path.exists(args.pbf):
        print(f"Error: PBF file not found: {args.pbf}")
        sys.exit(1)
    
    if not os.path.exists(args.config):
        print(f"Error: Config file not found: {args.config}")
        sys.exit(1)

    print(f"Loading config: {args.config}")
    config = load_config(args.config)

    # Priority: Config > CLI > Default
    chunk_size = config.get("chunk_size", args.chunk_size)
    print(f"Using chunk size: {chunk_size}")

    print(f"Ingesting OSM data: {args.pbf}")
    features, coastlines = ingest_osm(args.pbf, config)
    print(f"Extracted {len(features)} features and {len(coastlines)} coastlines.")

    if not features:
        print("No features found matching config. Exiting.")
        sys.exit(0)

    # Calculate global bounding box in microdegrees
    min_lon, min_lat, max_lon, max_lat = float('inf'), float('inf'), float('-inf'), float('-inf')
    for feat in features:
        f_minx, f_miny, f_maxx, f_maxy = feat["geometry"].bounds
        min_lon = min(min_lon, f_minx)
        min_lat = min(min_lat, f_miny)
        max_lon = max(max_lon, f_maxx)
        max_lat = max(max_lat, f_maxy)
    
    global_bbox = (
        int(min_lon * 1e6),
        int(min_lat * 1e6),
        int(max_lon * 1e6),
        int(max_lat * 1e6)
    )
    print(f"Global BBox: {global_bbox}")

    print("Building Quadtree index...")
    root = QuadtreeNode(global_bbox, chunk_size=chunk_size)
    for i, feat in enumerate(features):
        root.insert(feat)
        if i % 1000 == 0 and i > 0:
            print(f"Inserted {i} features...")

    print("Serializing to binary format...")
    binary_data = serialize_all(root, config, global_bbox, chunk_size=chunk_size)

    print(f"Writing to {args.output} ({len(binary_data)} bytes)...")
    with open(args.output, "wb") as f:
        f.write(binary_data)

    print("Done!")

if __name__ == "__main__":
    main()
