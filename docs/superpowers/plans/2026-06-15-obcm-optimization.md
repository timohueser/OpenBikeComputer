# OBCM Performance and Reliability Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix a critical file access bug in `obcm_view.py`, implement chunk decoding caching in `OBCMReader`, and optimize rendering by only re-calculating visible features when the viewport changes.

**Architecture:** 
1.  **File Management:** Ensure the file handle remains open while the `OBCMReader` is in use by moving the rendering loop inside the `with open` block.
2.  **Caching:** Use `functools.lru_cache` on `OBCMReader.decode_chunk` to reuse previously decoded chunk data.
3.  **Rendering Optimization:** Track viewport state (camera position and zoom) and only re-query the quadtree and re-decode chunks when the state changes.

**Tech Stack:** Python, Pygame, functools.

---

### Task 1: Update `obcm/reader.py` for Efficiency

**Files:**
- Modify: `obcm/reader.py`

- [ ] **Step 1: Add imports and caching**

```python
import struct
import io
import functools # ADD THIS

class OBCMReader:
    # ... existing methods ...

    @functools.lru_cache(maxsize=128) # ADD THIS
    def decode_chunk(self, chunk_id, node_bbox, chunk_size=4096):
        # node_bbox is a tuple, which is hashable, so it works with lru_cache
        # ...
```

- [ ] **Step 2: Verify syntax**
Run: `python3 -m py_compile obcm/reader.py`
Expected: No errors.

- [ ] **Step 3: Commit**
```bash
git add obcm/reader.py
git commit -m "perf(reader): add LRU cache to decode_chunk"
```

---

### Task 2: Fix `obcm_view.py` Critical Bug and Optimize Rendering

**Files:**
- Modify: `obcm_view.py`

- [ ] **Step 1: Move loop inside `with` and add viewport change detection**

```python
    with open(map_path, "rb") as f:
        reader = OBCMReader(f)
    
        # Init viewport at center of map
        min_lon, min_lat, max_lon, max_lat = reader.global_bbox
        vp = Viewport(1024, 768, (min_lat + max_lat) // 2)
        vp.camera_lon = (min_lon + max_lon) // 2
        vp.camera_lat = (min_lat + max_lat) // 2
        
        # Initial zoom: fit map to screen width
        vp.zoom = 1024 / (max_lon - min_lon) if max_lon != min_lon else 1.0
        
        panning = False
        
        last_vp_state = None # NEW
        visible_features = [] # NEW
        
        while True:
            for event in pygame.event.get():
                if event.type == pygame.QUIT:
                    pygame.quit()
                    sys.exit()
                # ... event handling (same as before) ...

            screen.fill((30, 30, 30))
            
            # Only re-query if viewport changed
            current_vp_state = (vp.camera_lon, vp.camera_lat, vp.zoom) # NEW
            if current_vp_state != last_vp_state: # NEW
                last_vp_state = current_vp_state
                # Calculate visible BBox
                v_min_lon, v_max_lat = vp.to_map(0, 0)
                v_max_lon, v_min_lat = vp.to_map(1024, 768)
                
                # Query visible chunks
                chunks = reader.query_bbox((v_min_lon, v_min_lat, v_max_lon, v_max_lat))
                visible_features = []
                for cid, node_bbox in chunks:
                    visible_features.extend(reader.decode_chunk(cid, node_bbox))
            
            # Draw visible features
            for f in visible_features: # CHANGED: use cached visible_features
                if f["style_id"] not in reader.styles:
                    continue
                style = reader.styles[f["style_id"]]
                
                # RGB565 to RGB888
                color = style["color"]
                r = (color >> 11) & 0x1F
                g = (color >> 5) & 0x3F
                b = color & 0x1F
                rgb = (r << 3, g << 2, b << 3)
                
                # Project points
                line_pts = [vp.to_screen(lon, lat) for lon, lat in f["points"]]
                if len(line_pts) >= 2:
                    pygame.draw.lines(screen, rgb, False, line_pts, max(1, style["weight"]))
            
            pygame.display.flip()
            clock.tick(60)
```

- [ ] **Step 2: Verify syntax**
Run: `python3 -m py_compile obcm_view.py`
Expected: No errors.

- [ ] **Step 3: Commit**
```bash
git add obcm_view.py
git commit -m "fix(visualizer): move main loop inside with block and optimize rendering"
```

---

### Task 3: Final Verification

- [ ] **Step 1: Run a smoke test (if possible in this environment)**
Since I cannot easily run Pygame in a headless environment, I will at least ensure there are no obvious runtime errors by checking the imports and class instantiation.

Run: `python3 -c "from obcm.reader import OBCMReader; import obcm_view"`
Expected: No errors.

- [ ] **Step 2: Final Check against requirements**
1. Move `while True` loop inside `with open`. (Done in Task 2)
2. Add LRU cache to `decode_chunk`. (Done in Task 1)
3. Optimize drawing by checking viewport changes. (Done in Task 2)
