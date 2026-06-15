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

    print(f"Ingesting OSM data: {args.pbf}")
    features = ingest_osm(args.pbf, config)
    print(f"Extracted {len(features)} features.")

    if not features:
        print("No features found matching config. Exiting.")
        sys.exit(0)

    # Calculate global bounding box in microdegrees
    all_lons = []
    all_lats = []
    for feat in features:
        for lon, lat in feat["geometry"].coords:
            all_lons.append(lon)
            all_lats.append(lat)
    
    global_bbox = (
        int(min(all_lons) * 1e6),
        int(min(all_lats) * 1e6),
        int(max(all_lons) * 1e6),
        int(max(all_lats) * 1e6)
    )
    print(f"Global BBox: {global_bbox}")

    print("Building Quadtree index...")
    root = QuadtreeNode(global_bbox, chunk_size=args.chunk_size)
    for i, feat in enumerate(features):
        root.insert(feat)
        if i % 1000 == 0 and i > 0:
            print(f"Inserted {i} features...")

    print("Serializing to binary format...")
    binary_data = serialize_all(root, config, global_bbox, chunk_size=args.chunk_size)

    print(f"Writing to {args.output} ({len(binary_data)} bytes)...")
    with open(args.output, "wb") as f:
        f.write(binary_data)

    print("Done!")

if __name__ == "__main__":
    main()
