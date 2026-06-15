# Design Spec: OBCM Interactive Visualizer

**Date:** 2026-06-15  
**Version:** 1.0  
**Status:** Approved  

## 1. Overview
The OBCM Visualizer is an interactive desktop tool built with Pygame to verify and display `.obcm` binary map files. It simulates the MCU's behavior by traversing the spatial index and decoding delta-compressed chunks on demand.

## 2. Core Requirements
- **Library:** Pygame.
- **Navigation:** 
    - Click and drag to pan the map.
    - Mouse wheel to zoom (centered on mouse pointer).
- **Aspect Correction:** Apply a constant `cos(latitude)` horizontal scale factor based on the map's center latitude to correct WGS84 stretching.
- **Data Loading:** Lazy loading of data chunks via the BFS Quadtree index.

## 3. Architecture

### 3.1 OBCMReader Class
- **File Parsing:** Reads the 29-byte global header.
- **Style Dictionary:** Loads the style lookup table into memory.
- **Index Management:** Loads the entire flattened Quadtree index (array of uint32) into memory.
- **Spatial Query:** 
    - Implements a recursive traversal of the BFS index.
    - Input: Bounding Box (microdegrees).
    - Output: List of `Chunk IDs` that intersect the BBox.
- **Data Decoding:**
    - Reads specific 4KB chunks from the file.
    - Decodes chained 8-bit or 16-bit deltas into absolute microdegree coordinates.
    - Maintains a LRU cache of decoded chunks to ensure smooth interaction.

### 3.2 Viewport & Projection
- **Coordinate Space:** Map (Microdegrees) <-> Screen (Pixels).
- **Projection Formula:**
    - `center_lat = (map_min_lat + map_max_lat) / 2`
    - `aspect_ratio = cos(radians(center_lat / 1e6))`
    - `screen_x = (lon - camera_lon) * zoom * aspect_ratio + offset_x`
    - `screen_y = (camera_lat - lat) * zoom + offset_y` (Note: Y is inverted in Pygame).

### 3.3 Main Loop
1. **Handle Events:** Panning, zooming, and window closing.
2. **Culling:** Calculate the microdegree BBox of the current screen.
3. **Fetching:** Query the `OBCMReader` for visible chunks.
4. **Drawing:** 
    - Iterate through visible features.
    - Select style (color, weight) from the Style Dictionary.
    - Project coordinates to screen pixels.
    - Use `pygame.draw.lines` for rendering.

## 4. Implementation Modules
- `obcm/reader.py`: The binary parsing and index traversal logic.
- `obcm_view.py`: The Pygame application and rendering loop.

## 5. Success Criteria
- Tool opens a window and renders an `.obcm` file correctly.
- Smooth panning and zooming.
- Visual confirmation that roads/features align correctly (no stretching or gaps).
- Proper interpretation of 8-bit vs 16-bit delta flags.
