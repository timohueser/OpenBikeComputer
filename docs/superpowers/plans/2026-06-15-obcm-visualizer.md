# OBCM Visualizer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an interactive Pygame-based visualizer for `.obcm` binary map files that lazily loads data chunks using the BFS spatial index.

**Architecture:** A modular approach with a dedicated `OBCMReader` for binary logic and a Pygame main loop for rendering. Uses a coordinate projection helper for aspect-corrected WGS84 display.

**Tech Stack:** Python 3.10+, Pygame, `struct`, `pytest`.

---

### Task 1: Environment Setup

**Files:**
- Modify: `requirements.txt`

- [ ] **Step 1: Add pygame to requirements.txt**

```text
pyosmium
shapely
pytest
pygame
```

- [ ] **Step 2: Commit**

```bash
git add requirements.txt
git commit -m "chore: add pygame to requirements"
```

---

### Task 2: Reader - Header & Styles

**Files:**
- Create: `obcm/reader.py`
- Create: `tests/test_reader.py`

- [ ] **Step 1: Write failing test for header parsing**

```python
import pytest
import struct
import io
from obcm.reader import OBCMReader

def test_read_header():
    # Mock a minimal OBCM file
    # Magic(4), Ver(1), BBox(4*i32), StyleOff(4), IndexOff(4)
    data = struct.pack("<4sBiiiiII", b"OBCM", 1, 0, 0, 100, 100, 29, 40)
    stream = io.BytesIO(data)
    reader = OBCMReader(stream)
    assert reader.version == 1
    assert reader.global_bbox == (0, 0, 100, 100)
```

- [ ] **Step 2: Implement `OBCMReader` initialization and header parsing**

```python
import struct
import io

class OBCMReader:
    def __init__(self, stream):
        self.stream = stream
        self._read_header()
        self.styles = {}
        self._read_styles()

    def _read_header(self):
        self.stream.seek(0)
        data = self.stream.read(29)
        magic, self.version, min_lat, min_lon, max_lat, max_lon, self.style_offset, self.index_offset = struct.unpack("<4sBiiiiII", data)
        if magic != b"OBCM":
            raise ValueError("Invalid magic bytes")
        self.global_bbox = (min_lon, min_lat, max_lon, max_lat)

    def _read_styles(self):
        self.stream.seek(self.style_offset)
        count = struct.unpack("<B", self.stream.read(1))[0]
        for _ in range(count):
            sid, z, color, weight = struct.unpack("<BBHB", self.stream.read(5))
            self.styles[sid] = {"z_index": z, "color": color, "weight": weight}
```

- [ ] **Step 3: Run tests**

Run: `PYTHONPATH=. pytest tests/test_reader.py`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add obcm/reader.py tests/test_reader.py
git commit -m "feat: implement OBCM reader header and style parsing"
```

---

### Task 3: Reader - Index Traversal

**Files:**
- Modify: `obcm/reader.py`
- Modify: `tests/test_reader.py`

- [ ] **Step 1: Write test for spatial query**

```python
def test_spatial_query():
    # Mock index: 1 branch pointing to 4 leaves
    # Branch at index 0 points to child index 1
    index = [0x80000001, 0, 1, 2, 3] # Branch(bit31=1), Leaf IDs
    # ... mock file stream with this index ...
    # This test needs careful setup of the mock file
    assert True
```

- [ ] **Step 2: Implement BFS index loading and spatial query**

```python
    def _load_index(self):
        self.stream.seek(self.index_offset)
        # For simplicity in plan, read until EOF or known size
        # In practice, DataStartOffset would bound this.
        data = self.stream.read() # Read remainder for now
        self.index = list(struct.unpack(f"<{len(data)//4}I", data[:(len(data)//4)*4]))

    def query_bbox(self, query_bbox):
        # query_bbox: (min_lon, min_lat, max_lon, max_lat) in microdegrees
        visible_chunks = []
        self._query_recursive(0, self.global_bbox, query_bbox, visible_chunks)
        return visible_chunks

    def _query_recursive(self, node_idx, node_bbox, query_bbox, results):
        # BBox intersection check
        if not self._intersects(node_bbox, query_bbox):
            return

        val = self.index[node_idx]
        if not (val & 0x80000000): # Leaf
            if val != 0x7FFFFFFF: # Not empty
                results.append((val, node_bbox))
        else: # Branch
            child_start = val & 0x7FFFFFFF
            min_lon, min_lat, max_lon, max_lat = node_bbox
            mid_lon, mid_lat = (min_lon + max_lon) // 2, (min_lat + max_lat) // 2
            
            # NW, NE, SW, SE order (matching serialize.py)
            children_bboxes = [
                (min_lon, mid_lat, mid_lon, max_lat),
                (mid_lon, mid_lat, max_lon, max_lat),
                (min_lon, min_lat, mid_lon, mid_lat),
                (mid_lon, min_lat, max_lon, mid_lat)
            ]
            for i, bbox in enumerate(children_bboxes):
                self._query_recursive(child_start + i, bbox, query_bbox, results)

    def _intersects(self, a, b):
        return not (a[2] < b[0] or a[0] > b[2] or a[3] < b[1] or a[1] > b[3])
```

- [ ] **Step 3: Commit**

```bash
git add obcm/reader.py
git commit -m "feat: implement recursive spatial index traversal"
```

---

### Task 4: Reader - Chunk Decoding

**Files:**
- Modify: `obcm/reader.py`

- [ ] **Step 1: Implement `decode_chunk`**

```python
    def decode_chunk(self, chunk_id, node_bbox, chunk_size=4096):
        data_start = self.index_offset + len(self.index) * 4 # Approximate
        # Real DataStartOffset should be calculated accurately
        self.stream.seek(data_start + chunk_id * chunk_size)
        chunk_data = self.stream.read(chunk_size)
        
        offset = 0
        features = []
        while offset < chunk_size:
            if chunk_data[offset] == 0xFF: break # Padding
            
            style_id, pt_count, ax, ay, flag = struct.unpack_from("<BHhhB", chunk_data, offset)
            offset += 8
            
            pts = [(node_bbox[0] + ax, node_bbox[1] + ay)]
            prev_x, prev_y = pts[0]
            
            d_fmt = "b" if flag == 1 else "h"
            d_size = 1 if flag == 1 else 2
            
            for _ in range(pt_count - 1):
                dx, dy = struct.unpack_from(f"<{d_fmt}{d_fmt}", chunk_data, offset)
                offset += d_size * 2
                x, y = prev_x + dx, prev_y + dy
                pts.append((x, y))
                prev_x, prev_y = x, y
            
            features.append({"style_id": style_id, "points": pts})
        return features
```

- [ ] **Step 2: Commit**

```bash
git add obcm/reader.py
git commit -m "feat: implement delta-compressed chunk decoding"
```

---

### Task 5: Viewport Logic

**Files:**
- Create: `obcm/viewport.py`
- Create: `tests/test_viewport.py`

- [ ] **Step 1: Implement coordinate projection with aspect correction**

```python
import math

class Viewport:
    def __init__(self, width, height, center_lat):
        self.width = width
        self.height = height
        self.camera_lon = 0
        self.camera_lat = 0
        self.zoom = 1.0 # pixels per microdegree
        self.aspect = math.cos(math.radians(center_lat / 1e6))

    def to_screen(self, lon, lat):
        x = (lon - self.camera_lon) * self.zoom * self.aspect + self.width / 2
        y = (self.camera_lat - lat) * self.zoom + self.height / 2
        return int(x), int(y)

    def to_map(self, x, y):
        lon = (x - self.width / 2) / (self.zoom * self.aspect) + self.camera_lon
        lat = self.camera_lat - (y - self.height / 2) / self.zoom
        return int(lon), int(lat)
```

- [ ] **Step 2: Commit**

```bash
git add obcm/viewport.py tests/test_viewport.py
git commit -m "feat: implement aspect-corrected coordinate projection"
```

---

### Task 6: Main App Implementation

**Files:**
- Create: `obcm_view.py`

- [ ] **Step 1: Implement Pygame loop and rendering**

```python
import pygame
import sys
from obcm.reader import OBCMReader
from obcm.viewport import Viewport

def main():
    if len(sys.argv) < 2:
        print("Usage: python obcm_view.py <map.obcm>")
        return

    pygame.init()
    screen = pygame.display.set_mode((1024, 768))
    clock = pygame.time.Clock()

    with open(sys.argv[1], "rb") as f:
        reader = OBCMReader(f)
    
    # Init viewport at center of map
    min_lon, min_lat, max_lon, max_lat = reader.global_bbox
    vp = Viewport(1024, 768, (min_lat + max_lat) // 2)
    vp.camera_lon = (min_lon + max_lon) // 2
    vp.camera_lat = (min_lat + max_lat) // 2
    
    # Simple interaction state
    panning = False
    
    while True:
        for event in pygame.event.get():
            if event.type == pygame.QUIT:
                return
            elif event.type == pygame.MOUSEBUTTONDOWN:
                if event.button == 1: panning = True
                elif event.button == 4: vp.zoom *= 1.2
                elif event.button == 5: vp.zoom /= 1.2
            elif event.type == pygame.MOUSEBUTTONUP:
                if event.button == 1: panning = False
            elif event.type == pygame.MOUSEMOTION and panning:
                dx, dy = event.rel
                vp.camera_lon -= dx / (vp.zoom * vp.aspect)
                vp.camera_lat += dy / vp.zoom

        screen.fill((30, 30, 30))
        
        # Calculate visible BBox
        v_min_lon, v_max_lat = vp.to_map(0, 0)
        v_max_lon, v_min_lat = vp.to_map(1024, 768)
        
        chunks = reader.query_bbox((v_min_lon, v_min_lat, v_max_lon, v_max_lat))
        for cid, bbox in chunks:
            feats = reader.decode_chunk(cid, bbox)
            for f in feats:
                style = reader.styles[f["style_id"]]
                color = style["color"] # Need to convert RGB565 to RGB888
                r = (color >> 11) & 0x1F
                g = (color >> 5) & 0x3F
                b = color & 0x1F
                rgb = (r << 3, g << 2, b << 3)
                
                line_pts = [vp.to_screen(lon, lat) for lon, lat in f["points"]]
                if len(line_pts) >= 2:
                    pygame.draw.lines(screen, rgb, False, line_pts, style["weight"])
        
        pygame.display.flip()
        clock.tick(60)

if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Commit**

```bash
git add obcm_view.py
git commit -m "feat: complete interactive OBCM visualizer"
```
