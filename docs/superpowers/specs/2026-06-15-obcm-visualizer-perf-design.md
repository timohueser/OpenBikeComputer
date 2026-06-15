# Design Doc: OBCM Visualizer Performance Metrics

## Overview
This document outlines the implementation of a performance monitoring overlay for the OBCM visualizer (`obcm_view.py`). The overlay will help developers analyze the performance impact of quadtree chunk sizes and spatial query complexity by displaying real-time timing metrics for the query and rendering phases.

## Goals
- Track time spent on spatial quadtree queries and feature decoding.
- Track time spent on Pygame primitive rendering.
- Display these metrics in a toggleable UI overlay.

## Architecture

### 1. State Management
The `debug_settings` dictionary will be extended.

```python
debug_settings = {
    "show_bboxes": False,
    "show_perf": False
}
```

A `perf_metrics` dictionary will store the last measured durations.

```python
perf_metrics = {
    "query_ms": 0.0,
    "render_ms": 0.0
}
```

### 2. Instrumentation
We will use `pygame.time.get_ticks()` or `time.perf_counter()` to measure durations.

- **Query Phase**: Wraps the `reader.query_bbox` and `reader.decode_chunk` loop.
- **Render Phase**: Wraps the "Draw visible features" loop.

### 3. Event Handling
- **Key 'T'**: Toggles `debug_settings["show_perf"]`.

### 4. UI Rendering
A text overlay in the bottom-right corner using `pygame.font`.
- **Location**: Bottom-right (offset by ~10px from edges).
- **Format**:
    - `Query: X.XX ms`
    - `Render: X.XX ms`
- **Styling**: White text on a semi-transparent black rectangle for readability.

## Data Flow
1. Main loop starts.
2. If viewport changed:
    - Start Timer -> Query/Decode -> End Timer -> Update `perf_metrics["query_ms"]`.
3. Start Timer -> Render Features -> End Timer -> Update `perf_metrics["render_ms"]`.
4. If `show_perf` is True:
    - Draw text overlay using `perf_metrics`.

## Testing & Verification
### Manual Verification
1. Run `python obcm_view.py map.obcm`.
2. Press 'T' to toggle performance stats.
3. Pan and zoom into high-density areas; verify metrics update dynamically.
4. Verify stats disappear when toggled off.
