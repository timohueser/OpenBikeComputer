# Design: Hybrid Shapefile Integration for Land Data

## Overview
This design replaces the complex, heuristic-based coastline parsing logic with a direct injection of pre-processed, high-quality land polygons sourced from OSMData shapefiles.

## Architecture
- **Dependency:** Added `fiona` to `requirements.txt` for efficient shapefile reading.
- **New Module (`obcm/land_ingest.py`)**:
    - **Fetcher**: Manages local caching (`~/.cache/obcm/`) of shapefiles. Automatically downloads/extracts if missing.
    - **Processor**: Uses `fiona` to read the shapefile, clips polygons to the target PBF bounding box, and converts them to `shapely.geometry.Polygon` objects.
- **`obcm_pack.py` Integration**:
    - Removes existing land generation heuristics.
    - Injects shapefile-sourced land polygons directly into the `features` list as first-class area features.
- **Serialization**: Reuses existing infrastructure for area serialization (same as lakes/parks).

## Workflow
1.  **Bounding Box**: Calculate input PBF bounding box.
2.  **Sourcing**: Use local cached shapefile or user-provided override.
3.  **Extraction**: Read and clip polygons from shapefile to PBF bounding box.
4.  **Injection**: Append land polygons to `features` list.
5.  **Quadtree**: Insert all features (including new land polygons) into the quadtree for efficient spatial indexing.

## Benefits
- **Robustness**: Eliminates complex, fragile coastline bridging and "land-side" heuristics.
- **Performance**: Direct polygon ingestion is significantly faster than polygonizing line segments.
- **Simplicity**: Treats land consistently with other area features like lakes.
