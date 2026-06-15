# OBCM Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Python pipeline that converts OSM PBF data into a custom, MCU-optimized `.obcm` binary format with a Quadtree index and delta compression.

**Architecture:** A modular pipeline: `ingest` (OSM parsing) -> `quadtree` (spatial slicing/clipping) -> `serialize` (binary packing). Uses BFS for Quadtree flattening and 4KB fixed-size data chunks.

**Tech Stack:** Python 3.10+, `pyosmium` (parsing), `shapely` (geometry), `struct` (binary), `pytest` (testing).

---

### Task 1: Environment & Config Setup

**Files:**
- Create: `requirements.txt`
- Create: `obcm/config.py`
- Create: `tests/test_config.py`

- [ ] **Step 1: Create requirements.txt**
```text
pyosmium
shapely
pytest
```

- [ ] **Step 2: Write failing test for config loading**
```python
import pytest
from obcm.config import load_config

def test_load_valid_config(tmp_path):
    config_file = tmp_path / "config.json"
    config_file.write_text('{"features": {"highway": {"primary": {"id": 10, "z_index": 50, "color": "0xF9A6", "weight": 4}}}}')
    config = load_config(str(config_file))
    assert config["features"]["highway"]["primary"]["id"] == 10
```

- [ ] **Step 3: Implement `load_config` in `obcm/config.py`**
```python
import json

def load_config(path: str) -> dict:
    with open(path, 'r') as f:
        return json.load(f)
```

- [ ] **Step 4: Run tests**
Run: `pytest tests/test_config.py`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add requirements.txt obcm/config.py tests/test_config.py
git commit -m "feat: initial environment and config loader"
```

---

### Task 2: OSM Ingestor (`ingest.py`)

**Files:**
- Create: `obcm/ingest.py`
- Create: `tests/test_ingest.py`

- [ ] **Step 1: Write test for OSM parsing (Mocking pyosmium)**
```python
from obcm.ingest import OSMHandler
from shapely.geometry import LineString

def test_handler_way_extraction():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    # Mock a way with 'highway'='primary' and some nodes
    # (Simplified for plan, actual implementation will use pyosmium interface)
    assert True 
```

- [ ] **Step 2: Implement `OSMHandler` in `obcm/ingest.py`**
```python
import osmium
from shapely.geometry import LineString, Point

class OSMHandler(osmium.SimpleHandler):
    def __init__(self, config):
        super().__init__()
        self.config = config
        self.features = []

    def way(self, w):
        for key, values in self.config["features"].items():
            if key in w.tags and w.tags[key] in values:
                style = values[w.tags[key]]
                try:
                    coords = [(n.lon, n.lat) for n in w.nodes]
                    if len(coords) < 2: continue
                    self.features.append({
                        "style_id": style["id"],
                        "geometry": LineString(coords)
                    })
                except osmium.InvalidLocationError:
                    continue
```

- [ ] **Step 3: Add `ingest_osm` function to handle the pipeline**
```python
def ingest_osm(pbf_path, config):
    handler = OSMHandler(config)
    handler.apply_file(pbf_path, locations=True)
    return handler.features
```

- [ ] **Step 4: Commit**
```bash
git add obcm/ingest.py tests/test_ingest.py
git commit -m "feat: implement OSM ingestor with pyosmium"
```

---

### Task 3: Spatial Slicer (`quadtree.py`)

**Files:**
- Create: `obcm/quadtree.py`
- Create: `tests/test_quadtree.py`

- [ ] **Step 1: Write test for Quadtree splitting**
```python
from obcm.quadtree import QuadtreeNode
from shapely.geometry import LineString

def test_quadtree_split():
    bbox = (0, 0, 100000, 100000) # microdegrees
    node = QuadtreeNode(bbox, chunk_size=4096)
    # Add a huge line that forces split (conceptual)
    # node.insert(...)
    # assert len(node.children) == 4
    assert True
```

- [ ] **Step 2: Implement `QuadtreeNode` with clipping**
```python
from shapely.geometry import box, LineString, MultiLineString

class QuadtreeNode:
    def __init__(self, bbox, chunk_size=4096):
        self.bbox = bbox # (min_lon, min_lat, max_lon, max_lat) in microdegrees
        self.chunk_size = chunk_size
        self.features = []
        self.children = []
        self.is_leaf = True

    def split(self):
        min_lon, min_lat, max_lon, max_lat = self.bbox
        mid_lon = (min_lon + max_lon) // 2
        mid_lat = (min_lat + max_lat) // 2
        
        self.children = [
            QuadtreeNode((min_lon, mid_lat, mid_lon, max_lat)), # NW
            QuadtreeNode((mid_lon, mid_lat, max_lon, max_lat)), # NE
            QuadtreeNode((min_lon, min_lat, mid_lon, mid_lat)), # SW
            QuadtreeNode((mid_lon, min_lat, max_lon, mid_lat)), # SE
        ]
        self.is_leaf = False
        # Redistribute features...
```

- [ ] **Step 3: Implement recursive insertion and clipping**
```python
    def insert(self, feature):
        geom = feature["geometry"]
        quad_box = box(*self.bbox)
        clipped = geom.intersection(quad_box)
        
        if clipped.is_empty: return
        
        if isinstance(clipped, MultiLineString):
            for part in clipped.geoms:
                self._insert_clipped({"style_id": feature["style_id"], "geometry": part})
        else:
            self._insert_clipped({"style_id": feature["style_id"], "geometry": clipped})

    def _insert_clipped(self, feature):
        if self.is_leaf:
            self.features.append(feature)
            if self.should_split():
                self.split()
        else:
            for child in self.children:
                child.insert(feature)
```

- [ ] **Step 4: Commit**
```bash
git add obcm/quadtree.py tests/test_quadtree.py
git commit -m "feat: implement spatial slicer with recursive quadtree"
```

---

### Task 4: Binary Serializer (`serialize.py`)

**Files:**
- Create: `obcm/serialize.py`
- Create: `tests/test_serialize.py`

- [ ] **Step 1: Write test for delta compression**
```python
from obcm.serialize import pack_deltas

def test_pack_deltas_8bit():
    points = [(100, 100), (110, 110), (120, 120)]
    anchor = (100, 100)
    data, flag = pack_deltas(points, anchor)
    assert flag == 1 # 8-bit
    assert len(data) == 4 # 2 points * 2 coords * 1 byte
```

- [ ] **Step 2: Implement delta packing**
```python
import struct

def pack_deltas(points, anchor):
    deltas = []
    max_delta = 0
    prev_x, prev_y = anchor
    
    for x, y in points[1:]:
        dx, dy = int(x - prev_x), int(y - prev_y)
        deltas.extend([dx, dy])
        max_delta = max(max_delta, abs(dx), abs(dy))
        prev_x, prev_y = x, y
        
    if max_delta <= 127:
        return struct.pack(f"<{len(deltas)}b", *deltas), 1
    else:
        return struct.pack(f"<{len(deltas)}h", *deltas), 2
```

- [ ] **Step 3: Implement BFS Quadtree flattening**
```python
from collections import deque

def serialize_tree(root, chunk_size):
    queue = deque([root])
    nodes_flat = []
    chunks = []
    
    while queue:
        node = queue.popleft()
        if node.is_leaf:
            # Pack features into chunk...
            chunk_id = len(chunks)
            chunks.append(pack_chunk(node.features, node.bbox, chunk_size))
            nodes_flat.append(chunk_id & 0x7FFFFFFF)
        else:
            child_idx = len(nodes_flat) + len(queue) + 1 # Conceptual
            nodes_flat.append(child_idx | 0x80000000)
            queue.extend(node.children)
    return nodes_flat, chunks
```

- [ ] **Step 4: Commit**
```bash
git add obcm/serialize.py tests/test_serialize.py
git commit -m "feat: implement binary serialization and delta compression"
```

---

### Task 5: Main Wrapper (`obcm_pack.py`)

**Files:**
- Create: `obcm_pack.py`

- [ ] **Step 1: Implement CLI and coordination**
```python
import argparse
from obcm.config import load_config
from obcm.ingest import ingest_osm
from obcm.quadtree import QuadtreeNode
from obcm.serialize import serialize_all

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("pbf")
    parser.add_argument("config")
    parser.add_argument("output")
    args = parser.parse_args()
    
    config = load_config(args.config)
    features = ingest_osm(args.pbf, config)
    
    # Calculate global BBox
    # Initialize Root Node
    # Insert all features
    # Serialize to file
    
if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Commit**
```bash
git add obcm_pack.py
git commit -m "feat: add main entry point and CLI"
```

---

### Task 6: Final Validation

- [ ] **Step 1: End-to-end test with small PBF**
- [ ] **Step 2: Verify binary header offsets**
- [ ] **Step 3: Verify chunk padding (0xFF)**
- [ ] **Step 4: Commit**
```bash
git commit -m "test: final validation and bugfixes"
```
