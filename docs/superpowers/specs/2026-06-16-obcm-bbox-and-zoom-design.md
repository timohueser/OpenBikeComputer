# OBCM Manual Bounding Box and Zoom Info Design

## Overview
This document outlines the design for adding a manual bounding box filter to the OBCM ingestion pipeline and a geographic scale indicator to the OBCM visualizer.

## 1. Manual Bounding Box (`obcm_pack.py` and `obcm/ingest.py`)

### Requirements
- Allow users to specify a specific rectangular area to extract from a larger `.osm.pbf` file during the packing process.
- Reduce the size of the resulting `.obcm` file by only including features within the specified area.

### Implementation Details
- **CLI Argument:** Add a `--bbox` argument to `obcm_pack.py` accepting four float values: `min_lon`, `min_lat`, `max_lon`, `max_lat`.
- **Filtering Logic:** 
  - If a bounding box is provided via the CLI, create a Shapely `box` geometry.
  - Modify `ingest_osm` in `obcm/ingest.py` to accept this optional bounding box.
  - Inside the `OSMHandler` (`way` and `area` methods), immediately after successfully constructing a Shapely `LineString` or `Polygon`, check for intersection using `geom.intersects(bbox)`.
  - Discard features that do not intersect.
- **Global Extent:** If a manual bounding box is provided, use it as the `global_bbox` for building the Quadtree, overriding the automatic calculation based on feature extents.

## 2. Zoom Level Info Overlay (`obcm_view.py`)

### Requirements
- Display the current geographic scale of the viewport in the bottom-left corner of the visualizer.

### Implementation Details
- **Distance Calculation:** Implement a Haversine formula helper function in `obcm_view.py` to calculate the distance in meters between two `(longitude, latitude)` points.
- **Viewport Dimensions:** During the rendering loop, use the `Viewport` to unproject the screen corners `(0, 0)` and `(SCREEN_WIDTH, SCREEN_HEIGHT)` into geographic coordinates.
- **Scale Calculation:**
  - Calculate the horizontal distance across the center of the screen (to account for latitude projection distortion).
  - Calculate the vertical distance across the center of the screen.
- **UI Overlay:** 
  - Format the distances (e.g., "View: 800m x 600m" or "View: 2.5km x 1.8km").
  - Render this text using a Pygame surface with a semi-transparent black background in the bottom-left corner, styling it similarly to the existing performance overlay.

## 3. File Size Context
The `.obcm` files are larger than their source `.osm.pbf` files by design. The `.osm.pbf` format is highly compressed for storage/transfer (zlib + varint delta encoding). The `.obcm` format prioritizes O(1) random access read speed for the visualizer, utilizing uncompressed, fixed-size chunks (with padding) and a spatial index, which inherently takes more disk space.
