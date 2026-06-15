# OSM Ingestor Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optimize OSM ingestor performance and robustness.

**Architecture:** Refactor `OSMHandler.way` for efficient tag lookup and robust geometry extraction. Update `ingest_osm` to use `flex_mem` cache.

**Tech Stack:** Python, pyosmium, shapely, pytest.

---

### Task 1: Add Edge Case Tests

**Files:**
- Modify: `tests/test_ingest.py`

- [ ] **Step 1: Write failing tests for edge cases**

```python
import pytest
import osmium
from unittest.mock import MagicMock
from obcm.ingest import OSMHandler
from shapely.geometry import LineString

# Existing test_handler_way_extraction remains...

def test_handler_way_too_few_nodes():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    mock_way = MagicMock()
    mock_way.tags = {"highway": "primary"}
    node1 = MagicMock(); node1.lon = 1.0; node1.lat = 1.0
    mock_way.nodes = [node1] # Only 1 node
    handler.way(mock_way)
    assert len(handler.features) == 0

def test_handler_way_no_matching_tags():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    mock_way = MagicMock()
    mock_way.tags = {"highway": "residential"} # Non-matching value
    node1 = MagicMock(); node1.lon = 1.0; node1.lat = 1.0
    node2 = MagicMock(); node2.lon = 2.0; node2.lat = 2.0
    mock_way.nodes = [node1, node2]
    handler.way(mock_way)
    assert len(handler.features) == 0

def test_handler_way_invalid_location_error():
    config = {"features": {"highway": {"primary": {"id": 10}}}}
    handler = OSMHandler(config)
    mock_way = MagicMock()
    mock_way.tags = {"highway": "primary"}
    
    # Mocking __iter__ to raise InvalidLocationError
    def mock_nodes_iter():
        raise osmium.InvalidLocationError()
        yield # Make it a generator
        
    mock_way.nodes.__iter__.side_effect = osmium.InvalidLocationError
    
    handler.way(mock_way)
    assert len(handler.features) == 0
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `PYTHONPATH=. pytest tests/test_ingest.py -v`
Expected: FAIL (some might pass by accident, but `test_handler_way_no_matching_tags` and `test_handler_way_invalid_location_error` should definitely pass or fail depending on current implementation, but we want to ensure they are handled by the *new* logic). Actually, `test_handler_way_invalid_location_error` might fail because of how we mock it if current code doesn't handle it well.

- [ ] **Step 3: Commit tests**

```bash
git add tests/test_ingest.py
git commit -m "test: add edge case tests for OSM ingestor"
```

### Task 2: Optimize `OSMHandler.way`

**Files:**
- Modify: `obcm/ingest.py`

- [ ] **Step 1: Implement optimized `way` method**

```python
    def way(self, w):
        style = None
        for tag_key, tag_val in w.tags:
            if tag_key in self.config["features"] and tag_val in self.config["features"][tag_key]:
                style = self.config["features"][tag_key][tag_val]
                break
        
        if not style:
            return

        try:
            coords = [(n.lon, n.lat) for n in w.nodes]
            if len(coords) < 2:
                return
            self.features.append({
                "style_id": style["id"],
                "geometry": LineString(coords)
            })
        except osmium.InvalidLocationError:
            return
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `PYTHONPATH=. pytest tests/test_ingest.py -v`
Expected: PASS

- [ ] **Step 3: Commit changes**

```bash
git add obcm/ingest.py
git commit -m "refactor: optimize OSMHandler.way feature lookup and robustness"
```

### Task 3: Update `ingest_osm` to use `flex_mem` cache

**Files:**
- Modify: `obcm/ingest.py`

- [ ] **Step 1: Update `apply_file` call**

```python
def ingest_osm(pbf_path, config):
    handler = OSMHandler(config)
    # Use locations=True to resolve node references to coordinates
    # Use flex_mem for efficient node location indexing
    handler.apply_file(pbf_path, locations=True, idx='flex_mem')
    return handler.features
```

- [ ] **Step 2: Verify tests still pass**

Run: `PYTHONPATH=. pytest tests/test_ingest.py -v`
Expected: PASS

- [ ] **Step 3: Commit changes**

```bash
git add obcm/ingest.py
git commit -m "refactor: use flex_mem cache in ingest_osm"
```
