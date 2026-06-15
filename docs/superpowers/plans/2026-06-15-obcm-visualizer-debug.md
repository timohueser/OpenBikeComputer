# OBCM Visualizer Debug Information Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a toggleable debug overlay to the OBCM visualizer that displays the bounding boxes of currently visible quadtree chunks.

**Architecture:** 
- Centralized `debug_settings` state within the main loop.
- Keyboard listener for the 'B' key to toggle bounding box visibility.
- Secondary rendering pass that projects and draws chunk boundaries after the map features are drawn.

**Tech Stack:** Python, Pygame, OBCM Internal API (`Viewport`, `OBCMReader`)

---

### Task 1: Initialize Debug State and Event Handling

**Files:**
- Modify: `obcm_view.py`

- [ ] **Step 1: Define debug settings state**

Add `debug_settings` just before the `while True` loop in `main()`.

```python
        # Initial zoom: fit map to screen width
        vp.zoom = SCREEN_WIDTH / (max_lon - min_lon) if max_lon != min_lon else 1.0
        
        debug_settings = {
            "show_bboxes": False
        }
        
        panning = False
```

- [ ] **Step 2: Add keyboard event listener**

Update the event loop to handle `pygame.KEYDOWN` and the 'B' key.

```python
                elif event.type == pygame.MOUSEMOTION and panning:
                    dx, dy = event.rel
                    # Convert screen delta to map delta
                    vp.camera_lon -= dx / (vp.zoom * vp.aspect)
                    vp.camera_lat += dy / vp.zoom
                elif event.type == pygame.KEYDOWN:
                    if event.key == pygame.K_b:
                        debug_settings["show_bboxes"] = not debug_settings["show_bboxes"]
                        print(f"Debug: show_bboxes = {debug_settings['show_bboxes']}")
```

- [ ] **Step 3: Commit**

```bash
git add obcm_view.py
git commit -m "feat(visualizer): add debug state and 'B' key toggle"
```

---

### Task 2: Implement Bounding Box Rendering

**Files:**
- Modify: `obcm_view.py`

- [ ] **Step 1: Add debug overlay rendering pass**

Insert the bounding box drawing logic after the feature drawing loop, but before `pygame.display.flip()`.

```python
            # Draw visible features
            for f in visible_features:
                # ... (existing drawing logic) ...
                if len(line_pts) >= 2:
                    pygame.draw.lines(screen, rgb, False, line_pts, max(1, style["weight"]))
            
            # Debug Overlay
            if debug_settings["show_bboxes"]:
                for cid, node_bbox in chunks:
                    # node_bbox: (min_lon, min_lat, max_lon, max_lat)
                    min_lon, min_lat, max_lon, max_lat = node_bbox
                    
                    # Project corners
                    # Top-Left, Top-Right, Bottom-Right, Bottom-Left
                    points = [
                        vp.to_screen(min_lon, max_lat),
                        vp.to_screen(max_lon, max_lat),
                        vp.to_screen(max_lon, min_lat),
                        vp.to_screen(min_lon, min_lat)
                    ]
                    
                    # Draw bright green rectangle (0, 255, 0)
                    pygame.draw.lines(screen, (0, 255, 0), True, points, 1)
            
            pygame.display.flip()
```

- [ ] **Step 2: Commit**

```bash
git add obcm_view.py
git commit -m "feat(visualizer): render debug bounding boxes when enabled"
```

---

### Task 3: Manual Verification

**Files:**
- None (Execution only)

- [ ] **Step 1: Run visualizer and verify toggle**

Run the visualizer with an existing `.obcm` file.

Command: `python obcm_view.py tests/test_data/sample.obcm` (or any valid .obcm file)

- Verify map renders normally.
- Press 'B'. Verify green boxes appear around clusters of features.
- Pan and zoom. Verify boxes stay aligned with the features.
- Press 'B' again. Verify boxes disappear.

- [ ] **Step 2: Final Commit (if any cleanups needed)**

```bash
git commit -m "docs: finalize visualizer debug feature" --allow-empty
```
