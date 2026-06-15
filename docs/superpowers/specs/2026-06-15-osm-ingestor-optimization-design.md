# OSM Ingestor Optimization Design

## Goal
Optimize the OSM ingestor for performance and robustness by improving feature lookup efficiency, using a high-performance node cache, and handling edge cases.

## Proposed Changes

### 1. Inefficient Feature Lookup Optimization
In `OSMHandler.way`, the current implementation iterates over the entire `config["features"]`. For each config entry, it checks if the key exists in way tags. 
**Optimization:** Iterate through the way's tags and check if the tag key and value exist in the `config["features"]`. This is generally faster as ways typically have fewer tags than the global configuration has feature definitions.

### 2. FlexMem Node Cache
Update `ingest_osm` to use `flex_mem` node cache. 
**Change:** `handler.apply_file(pbf_path, locations=True, idx='flex_mem')`.

### 3. Robustness and Edge Case Handling
- **Coordinate Extraction:** Only extract coordinates if a matching style is found.
- **Node Count Validation:** Ignore ways with fewer than 2 nodes (invalid for `LineString`).
- **Error Handling:** Gracefully handle `osmium.InvalidLocationError`.

### 4. Test Coverage Expansion
Add tests for:
- Ways with < 2 nodes (should be ignored).
- Ways with non-matching tags (should be ignored).
- Handling of `osmium.InvalidLocationError`.

## Architecture
- `OSMHandler` (in `obcm/ingest.py`): Modified `way` method with optimized lookup and validation logic.
- `ingest_osm` (in `obcm/ingest.py`): Updated `apply_file` call with `idx='flex_mem'`.
- `tests/test_ingest.py`: New test cases for edge cases.

## Success Criteria
- All tests pass, including new edge case tests.
- OSM lookup logic is more efficient.
- `flex_mem` cache is used.
