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

    if not features and not coastlines:
        print("No features found matching config. Exiting.")
        sys.exit(0)

    # Calculate global bounding box in degrees
    min_lon, min_lat, max_lon, max_lat = float('inf'), float('inf'), float('-inf'), float('-inf')
    for feat in features:
        f_minx, f_miny, f_maxx, f_maxy = feat["geometry"].bounds
        min_lon = min(min_lon, f_minx)
        min_lat = min(min_lat, f_miny)
        max_lon = max(max_lon, f_maxx)
        max_lat = max(max_lat, f_maxy)
    
    for cl in coastlines:
        f_minx, f_miny, f_maxx, f_maxy = cl.bounds
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

    # --- Land Generation Logic ---
    if coastlines and "natural" in config.get("features", {}) and "land" in config["features"]["natural"]:
        from shapely.geometry import box, LineString, Point
        from shapely.ops import linemerge, polygonize, unary_union, nearest_points
        
        print(f"Generating Land polygon from {len(coastlines)} coastline segments...")
        land_style = config["features"]["natural"]["land"]["id"]
        
        # Strictly bound by the data extent
        bbox_poly = box(min_lon, min_lat, max_lon, max_lat)
        bbox_boundary = bbox_poly.boundary
        
        merged_coastlines = linemerge(coastlines)
        if merged_coastlines.geom_type == 'LineString':
            merged_parts = [merged_coastlines]
        elif hasattr(merged_coastlines, 'geoms'):
            merged_parts = list(merged_coastlines.geoms)
        else:
            merged_parts = []
            
        # 1. Bridge small gaps between coastline segments (fixes broken islands)
        # 2. Connect remaining open ends to the boundary if they are near the edge
        connectors = []
        open_ends = []
        
        for line in merged_parts:
            if not line.is_closed:
                open_ends.append(Point(line.coords[0]))
                open_ends.append(Point(line.coords[-1]))
                
        # Find pairs of open ends that are close to each other
        used_ends = set()
        GAP_TOLERANCE = 0.002 # ~200m tolerance for broken coastline data
        
        for i, p1 in enumerate(open_ends):
            if i in used_ends: continue
            
            # Find closest other endpoint
            best_j = -1
            min_dist = GAP_TOLERANCE
            
            for j, p2 in enumerate(open_ends):
                if i == j or j in used_ends: continue
                dist = p1.distance(p2)
                if dist < min_dist:
                    min_dist = dist
                    best_j = j
                    
            if best_j != -1:
                connectors.append(LineString([p1, open_ends[best_j]]))
                used_ends.add(i)
                used_ends.add(best_j)
            else:
                # If no other endpoint is near, connect to the bounding box
                _, np = nearest_points(p1, bbox_boundary)
                connectors.append(LineString([p1, np]))
                used_ends.add(i)
        
        # Union everything: coastlines, connectors, and the box boundary
        all_lines = unary_union(merged_parts + connectors + [bbox_boundary])
        
        # Find all enclosed areas
        all_polygons = list(polygonize(all_lines))
        print(f"Polygonized into {len(all_polygons)} potential areas.")
        
        # Heuristic: Land is on the LEFT of OSM coastlines.
        # We'll use a positive offset curve to find a point that is definitely on the land side.
        land_test_points = []
        for l in merged_parts:
            offset_line = l.offset_curve(0.0001)
            if not offset_line.is_empty:
                if offset_line.geom_type == 'LineString':
                    idx = len(offset_line.coords) // 2
                    land_test_points.append(Point(offset_line.coords[idx]))
                elif hasattr(offset_line, 'geoms') and len(offset_line.geoms) > 0:
                    part = offset_line.geoms[0]
                    idx = len(part.coords) // 2
                    land_test_points.append(Point(part.coords[idx]))
        
        added_count = 0
        for i, poly in enumerate(all_polygons):
            is_land = False
            for p in land_test_points:
                if poly.contains(p):
                    is_land = True
                    break
            
            if is_land:
                features.append({"style_id": land_style, "geometry": poly})
                added_count += 1
            else:
                print(f"  - Polygon {i} identified as Sea (skipped).")
        
        print(f"Successfully added {added_count} land polygons.")

        # Also add raw coastlines for debugging if requested in config
        if "coastline_debug" in config["features"]["natural"]:
            debug_style = config["features"]["natural"]["coastline_debug"]["id"]
            for cl in coastlines:
                features.append({"style_id": debug_style, "geometry": cl})
            print(f"Added {len(coastlines)} debug coastline segments.")

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
