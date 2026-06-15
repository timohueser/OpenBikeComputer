# Configurable Chunk Size Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the quadtree chunk size configurable via `config.json` and store it in the `.obcm` file header so the reader can automatically adapt.

**Architecture:** 
- Update `.obcm` header format to include `uint16 chunk_size`.
- Modify `serialize_all` to write the new header.
- Modify `obcm_pack.py` to read `chunk_size` from config.
- Modify `OBCMReader` to parse and use the new header field.

**Tech Stack:** Python, Struct, JSON

---

### Task 1: Update Serialization and Header Format

**Files:**
- Modify: `obcm/serialize.py`
- Modify: `tests/test_serialize.py`

- [ ] **Step 1: Update `serialize_all` to include chunk_size in header**

Update the header format string and packing logic.

```python
    # Header: Magic(4), Version(1), BBox(4x i32), StyleOff(4), IndexOff(4), ChunkSize(2)
    style_offset = 31 # Header is now 31 bytes
    index_offset = style_offset + len(style_data)
    
    header = struct.pack("<4sBiiiiIIH",
                        b"OBCM",
                        0x01,
                        global_bbox[1], global_bbox[0], global_bbox[3], global_bbox[2],
                        style_offset,
                        index_offset,
                        chunk_size)
```

- [ ] **Step 2: Update tests for new header size**

Modify `tests/test_serialize.py` (if any tests check header size directly, though they seem more focused on internal packing). Let's add a new test case for the full serialization header.

```python
def test_serialize_all_header_size():
    from obcm.serialize import serialize_all
    class MockNode:
        def __init__(self):
            self.is_leaf = True
            self.features = []
            self.bbox = (0, 0, 100, 100)
    
    root = MockNode()
    config = {"features": {}}
    global_bbox = (0, 0, 100, 100)
    
    binary = serialize_all(root, config, global_bbox, chunk_size=2048)
    # Header(31) + StyleCount(1) + Index(4) + Chunk(2048) = 2084
    # Wait, if no features, data_chunks is empty.
    # Header(31) + StyleCount(1) + Index(4) = 36
    assert len(binary) == 36
    magic, ver, lat1, lon1, lat2, lon2, s_off, i_off, c_size = struct.unpack("<4sBiiiiIIH", binary[:31])
    assert c_size == 2048
```

- [ ] **Step 3: Commit**

```bash
git add obcm/serialize.py tests/test_serialize.py
git commit -m "feat(serialize): include chunk_size in .obcm header"
```

---

### Task 2: Update Packer Config Handling

**Files:**
- Modify: `obcm_pack.py`

- [ ] **Step 1: Update `main` to prioritize config chunk_size**

```python
    print(f"Loading config: {args.config}")
    config = load_config(args.config)
    
    # Priority: Config > CLI > Default
    chunk_size = config.get("chunk_size", args.chunk_size)
    print(f"Using chunk size: {chunk_size}")

    print(f"Ingesting OSM data: {args.pbf}")
    # ...
    root = QuadtreeNode(global_bbox, chunk_size=chunk_size)
    # ...
    binary_data = serialize_all(root, config, global_bbox, chunk_size=chunk_size)
```

- [ ] **Step 2: Commit**

```bash
git add obcm_pack.py
git commit -m "feat(packer): allow configuring chunk_size via config.json"
```

---

### Task 3: Update Reader to use Header ChunkSize

**Files:**
- Modify: `obcm/reader.py`
- Modify: `tests/test_reader.py`

- [ ] **Step 1: Update `_read_header` to parse ChunkSize**

```python
    def _read_header(self):
        self.stream.seek(0)
        data = self.stream.read(31)
        if len(data) < 31:
            raise ValueError("File too short for header")
        magic, self.version, min_lat, min_lon, max_lat, max_lon, self.style_offset, self.index_offset, self.chunk_size = struct.unpack("<4sBiiiiIIH", data)
        # ...
```

- [ ] **Step 2: Update `_decode_chunk` to use self.chunk_size**

```python
    def _decode_chunk(self, chunk_id, node_bbox): # Remove chunk_size=4096 default
        """
        Reads and decodes a data chunk from the file.
        """
        if not hasattr(self, 'index'):
            self._load_index()

        # Data starts immediately after the Index Block
        data_start_offset = self.index_offset + (len(self.index) * 4)
        
        self.stream.seek(data_start_offset + chunk_id * self.chunk_size)
        chunk_data = self.stream.read(self.chunk_size)
        
        offset = 0
        chunk_size = self.chunk_size # Local alias for compatibility with existing loop
        # ... rest of the function ...
```

- [ ] **Step 3: Update existing tests to 31-byte header**

Modify `tests/test_reader.py` to include the `uint16` chunk size in all mock headers.

```python
def test_read_header():
    # MinLat=10, MinLon=20, MaxLat=30, MaxLon=40, ChunkSize=4096
    data = struct.pack("<4sBiiiiIIH", b"OBCM", 1, 10, 20, 30, 40, 31, 40, 4096)
    stream = io.BytesIO(data)
    reader = OBCMReader(stream)
    assert reader.chunk_size == 4096
```

- [ ] **Step 4: Commit**

```bash
git add obcm/reader.py tests/test_reader.py
git commit -m "feat(reader): dynamically read chunk_size from header"
```

---

### Task 4: Final Verification

- [ ] **Step 1: Run all tests**

```bash
PYTHONPATH=. pytest
```

- [ ] **Step 2: Manual verification with custom chunk size**

1. Create a config with `"chunk_size": 1024`.
2. Pack a small PBF.
3. Verify visualizer opens it and the 'B' toggle shows smaller boxes.
