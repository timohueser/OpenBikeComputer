# Design Doc: OBCM Visualizer Debug Information

## Overview
This document outlines the implementation of a debug visualization layer for the OBCM (Open Bike Computer Map) visualizer (`obcm_view.py`). The primary goal is to allow developers to visualize the spatial partitioning (quadtree chunks) of the map data to verify ingest efficiency and spatial query correctness.

## Goals
- Provide a toggleable debug overlay.
- Display bounding boxes for all currently visible quadtree leaf nodes.
- Establish a pattern for adding future debug visualizations.

## Architecture

### 1. State Management
A `debug_settings` dictionary will be introduced in the `main` loop of `obcm_view.py`.

```python
debug_settings = {
    "show_bboxes": False
}
```

### 2. Event Handling
The `pygame` event loop will be extended to handle keyboard input.
- **Key 'B'**: Toggles `debug_settings["show_bboxes"]`.

### 3. Rendering Logic
The rendering phase will be split into two passes:
1. **Map Pass**: Draws visible features (lines/polygons) as it does currently.
2. **Debug Pass**: If `debug_settings["show_bboxes"]` is True:
    - Iterate through `chunks` (returned from `reader.query_bbox`).
    - Project the `node_bbox` corners to screen coordinates using the `Viewport`.
    - Draw a non-filled rectangle using `pygame.draw.rect` or `pygame.draw.lines`.
    - Color: Bright Green (`0x07E0` in RGB565 / `(0, 255, 0)` in RGB888).

## Data Flow
1. User presses 'B'.
2. `debug_settings["show_bboxes"]` is toggled.
3. `pygame` main loop continues.
4. If toggled ON, the next frame will calculate and draw rectangles based on the `chunks` list already generated for feature querying.

## Future Extensibility
The `debug_settings` dictionary can be easily expanded with:
- `show_point_counts`: Display number of points in each chunk.
- `show_node_ids`: Display the internal quadtree index for each chunk.
- `show_raw_gps`: Overlay GPS trace data for comparison.

## Testing & Verification
### Manual Verification
1. Run `python obcm_view.py map.obcm`.
2. Press 'B' to toggle boxes.
3. Verify boxes align with the spatial extent of features within them.
4. Pan and zoom to ensure boxes are correctly re-projected.
5. Toggle OFF to ensure clean map view.
