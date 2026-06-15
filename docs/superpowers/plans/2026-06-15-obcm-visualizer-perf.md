# OBCM Visualizer Performance Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a toggleable performance overlay to the OBCM visualizer that displays real-time timing metrics for the quadtree query and feature rendering phases.

**Architecture:** 
- Centralized `perf_metrics` dictionary to store phase durations.
- Keyboard listener for the 'T' key to toggle performance visibility.
- Instrumentation using `time.perf_counter()` for high-precision timing.
- Text rendering overlay in the bottom-right corner using Pygame's font module.

**Tech Stack:** Python, Pygame, Time

---

### Task 1: Initialize Performance State and Event Handling

**Files:**
- Modify: `obcm_view.py`

- [ ] **Step 1: Import `time` module and update `debug_settings`**

Add `import time` and initialize `perf_metrics`.

```python
import pygame
import sys
import os
import math
import time  # New import
from obcm.reader import OBCMReader
# ... (rest of imports)
```

In `main()`:
```python
        debug_settings = {
            "show_bboxes": False,
            "show_perf": False  # New key
        }
        
        perf_metrics = {
            "query_ms": 0.0,
            "render_ms": 0.0
        }
        
        pygame.font.init()
        font = pygame.font.SysFont("monospace", 20)
```

- [ ] **Step 2: Add 'T' key event listener**

```python
                elif event.type == pygame.KEYDOWN:
                    if event.key == pygame.K_b:
                        debug_settings["show_bboxes"] = not debug_settings["show_bboxes"]
                        print(f"Debug: show_bboxes = {debug_settings['show_bboxes']}")
                    elif event.key == pygame.K_t:
                        debug_settings["show_perf"] = not debug_settings["show_perf"]
                        print(f"Debug: show_perf = {debug_settings['show_perf']}")
```

- [ ] **Step 3: Commit**

```bash
git add obcm_view.py
git commit -m "feat(visualizer): add performance toggle state and event listener"
```

---

### Task 2: Implement Performance Instrumentation

**Files:**
- Modify: `obcm_view.py`

- [ ] **Step 1: Instrument the Query Phase**

Wrap the viewport query logic.

```python
            # Only re-query if viewport changed
            current_vp_state = (vp.camera_lon, vp.camera_lat, vp.zoom)
            if current_vp_state != last_vp_state:
                t0 = time.perf_counter() # Start Query Timer
                last_vp_state = current_vp_state
                # Calculate visible BBox
                v_min_lon, v_max_lat = vp.to_map(0, 0)
                v_max_lon, v_min_lat = vp.to_map(SCREEN_WIDTH, SCREEN_HEIGHT)
                
                # Query visible chunks
                chunks = reader.query_bbox((v_min_lon, v_min_lat, v_max_lon, v_max_lat))
                visible_features = []
                for cid, node_bbox in chunks:
                    visible_features.extend(reader.decode_chunk(cid, node_bbox))
                
                perf_metrics["query_ms"] = (time.perf_counter() - t0) * 1000.0 # Update metric
```

- [ ] **Step 2: Instrument the Render Phase**

Wrap the feature rendering loop.

```python
            t_render_start = time.perf_counter() # Start Render Timer
            # Draw visible features
            for f in visible_features:
                # ... (existing drawing logic) ...
            
            # Debug Overlay (Bounding Boxes)
            if debug_settings["show_bboxes"]:
                # ... (existing bbox logic) ...
            
            perf_metrics["render_ms"] = (time.perf_counter() - t_render_start) * 1000.0 # Update metric
```

- [ ] **Step 3: Commit**

```bash
git add obcm_view.py
git commit -m "feat(visualizer): instrument query and render loops"
```

---

### Task 3: Implement UI Overlay

**Files:**
- Modify: `obcm_view.py`

- [ ] **Step 1: Render the performance overlay**

Add the text rendering logic at the end of the debug overlay section.

```python
            # Performance Overlay
            if debug_settings["show_perf"]:
                labels = [
                    f"Query:  {perf_metrics['query_ms']:.2f} ms",
                    f"Render: {perf_metrics['render_ms']:.2f} ms"
                ]
                
                for i, text in enumerate(labels):
                    surf = font.render(text, True, (255, 255, 255))
                    # Background rect for readability
                    bg_rect = surf.get_rect()
                    bg_rect.bottomright = (SCREEN_WIDTH - 10, SCREEN_HEIGHT - 10 - (i * 25))
                    pygame.draw.rect(screen, (0, 0, 0, 150), bg_rect.inflate(10, 5))
                    screen.blit(surf, bg_rect)
```

- [ ] **Step 2: Commit**

```bash
git add obcm_view.py
git commit -m "feat(visualizer): add performance UI overlay"
```

---

### Task 4: Final Verification

- [ ] **Step 1: Run and verify metrics**

1. Run the visualizer.
2. Press 'T'. Verify "Query" and "Render" times appear in bottom-right.
3. Pan around. Verify Query time spikes when new chunks are loaded, and Render time spikes in dense areas.
4. Press 'T' again. Verify overlay disappears.
