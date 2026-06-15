# Quadtree Optimization and Geometry Handling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optimize `QuadtreeNode` for efficiency and robustness by pre-calculating boundaries, maintaining size state, and handling complex geometry types with a recursion guard.

**Architecture:** Refactor `QuadtreeNode` to move expensive calculations to `__init__`, track aggregate feature size incrementally, and implement a recursive geometry flattener with a microdegree-scale depth limit.

**Tech Stack:** Python, Shapely

---

### Task 1: Refactor `__init__` and State Management

**Files:**
- Modify: `obcm/quadtree.py`

- [ ] **Step 1: Update `__init__` to pre-calculate float boundaries and `q_box`**

```python
    def __init__(self, bbox, chunk_size=4096):
        # bbox: (min_lon, min_lat, max_lon, max_lat) in microdegrees
        self.bbox = bbox
        self.chunk_size = chunk_size
        self.features = []
        self.children = []
        self.is_leaf = True
        
        # Pre-calculate float boundaries and shapely box
        self.min_lon_f, self.min_lat_f, self.max_lon_f, self.max_lat_f = [c / 1e6 for c in self.bbox]
        self.q_box = box(self.min_lon_f, self.min_lat_f, self.max_lon_f, self.max_lat_f)
        
        # Incremental size tracking
        self.current_size = 0
```

- [ ] **Step 2: Update `should_split` to use `self.current_size`**

```python
    def should_split(self):
        width = self.bbox[2] - self.bbox[0]
        height = self.bbox[3] - self.bbox[1]
        
        # Split if too large in dimensions
        if width > 32767 or height > 32767:
            return True
        
        # Split if too many points/data
        return self.current_size > self.chunk_size
```

- [ ] **Step 3: Update `_process_clipped` to update `self.current_size`**

```python
    def _process_clipped(self, feature):
        if self.is_leaf:
            self.features.append(feature)
            # Update current_size incrementally
            pt_count = len(feature["geometry"].coords)
            self.current_size += 8 + (pt_count * 4)
            
            if self.should_split():
                self.split()
        else:
            for child in self.children:
                child.insert(feature)
```

- [ ] **Step 4: Verify existing tests pass**

Run: `pytest tests/test_quadtree.py`

- [ ] **Step 5: Commit changes**

```bash
git add obcm/quadtree.py
git commit -m "perf: pre-calculate boundaries and track quadtree size incrementally"
```

### Task 2: Implement Robust Geometry Handling and Recursion Guard

**Files:**
- Modify: `obcm/quadtree.py`

- [ ] **Step 1: Implement `_flatten_and_process` and update `insert`**

```python
    def insert(self, feature):
        clipped = feature["geometry"].intersection(self.q_box)
        if clipped.is_empty:
            return

        self._flatten_and_process(clipped, feature["style_id"])

    def _flatten_and_process(self, geom, style_id):
        if geom.is_empty:
            return
        
        if hasattr(geom, 'geoms'): # MultiLineString, MultiPolygon, GeometryCollection
            for part in geom.geoms:
                self._flatten_and_process(part, style_id)
        elif geom.geom_type in ['LineString', 'LinearRing']:
            self._process_clipped({"style_id": style_id, "geometry": geom})
        elif geom.geom_type == 'Polygon':
            self._flatten_and_process(geom.exterior, style_id)
            for interior in geom.interiors:
                self._flatten_and_process(interior, style_id)
```

- [ ] **Step 2: Add recursion guard to `split`**

```python
    def split(self):
        min_lon, min_lat, max_lon, max_lat = self.bbox
        width = max_lon - min_lon
        height = max_lat - min_lat
        
        # Recursion guard: Don't split if smaller than 10 microdegrees
        if width < 10 or height < 10:
            return

        mid_lon = (min_lon + max_lon) // 2
        mid_lat = (min_lat + max_lat) // 2
        
        # ... rest of split logic
```

- [ ] **Step 3: Verify with existing tests**

Run: `pytest tests/test_quadtree.py`

- [ ] **Step 4: Commit changes**

```bash
git add obcm/quadtree.py
git commit -m "refactor: robust geometry handling and recursion guard"
```

### Task 3: Update Tests for New Requirements

**Files:**
- Modify: `tests/test_quadtree.py`

- [ ] **Step 1: Add test for Polygon handling**

```python
def test_quadtree_polygon_handling():
    from shapely.geometry import Polygon
    node = QuadtreeNode((0, 0, 1000, 1000))
    poly = Polygon([(0.0001, 0.0001), (0.0005, 0.0001), (0.0005, 0.0005), (0.0001, 0.0005)])
    node.insert({"style_id": 1, "geometry": poly})
    # Should extract exterior as LineString
    assert len(node.features) == 1
    assert node.features[0]["geometry"].geom_type == 'LinearRing' or node.features[0]["geometry"].geom_type == 'LineString'
```

- [ ] **Step 2: Add test for Recursion Guard**

```python
def test_quadtree_recursion_guard():
    # Force split on a very small box
    node = QuadtreeNode((0, 0, 8, 8), chunk_size=1)
    line = LineString([(0, 0), (0.000005, 0.000005)])
    node.insert({"style_id": 1, "geometry": line})
    # Should NOT split because dimension < 10
    assert node.is_leaf == True
```

- [ ] **Step 3: Add test for GeometryCollection**

```python
def test_quadtree_geometry_collection():
    from shapely.geometry import GeometryCollection, Point
    node = QuadtreeNode((0, 0, 1000, 1000))
    gc = GeometryCollection([
        LineString([(0.0001, 0.0001), (0.0002, 0.0002)]),
        Point(0.0005, 0.0005) # Should be ignored
    ])
    node.insert({"style_id": 1, "geometry": gc})
    assert len(node.features) == 1
```

- [ ] **Step 4: Run all tests**

Run: `pytest tests/test_quadtree.py`

- [ ] **Step 5: Commit changes**

```bash
git add tests/test_quadtree.py
git commit -m "test: add coverage for polygons, geometry collections, and recursion guard"
```
