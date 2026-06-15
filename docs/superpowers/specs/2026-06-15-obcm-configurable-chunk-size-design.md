# Design Doc: Configurable Chunk Size

## Overview
This document describes the changes required to make the quadtree chunk size configurable via the project's `config.json` and to store this parameter within the `.obcm` binary file header. This ensures the visualizer and MCU firmware can correctly parse the data regardless of the chosen chunk size at export time.

## Goals
- Allow users to define `"chunk_size"` in `config.json`.
- Store the `chunk_size` in the `.obcm` file header (breaking change).
- Ensure the `OBCMReader` dynamically adapts to the chunk size stored in the file.

## Architecture

### 1. File Format Update
The header size increases from 29 to 31 bytes.
- **Format String**: `<4sBiiiiIIH`
- **Fields**:
  - Magic: `4s` (OBCM)
  - Version: `B` (0x01)
  - BBox: `iiii` (min_lat, min_lon, max_lat, max_lon)
  - Style Offset: `I` (uint32)
  - Index Offset: `I` (uint32)
  - **Chunk Size**: `H` (uint16) - **NEW FIELD**

### 2. Packer Changes (`obcm_pack.py` & `obcm/serialize.py`)
- **Config Loading**: `obcm_pack.py` will prioritize `"chunk_size"` from the JSON config.
- **CLI Fallback**: If not in config, it uses the `--chunk-size` CLI argument (default 4096).
- **Serialization**: `serialize_all` will use the provided `chunk_size` to calculate the `style_offset` (now starting at 31) and pack the header with the new field.

### 3. Reader Changes (`obcm/reader.py`)
- **Header Parsing**: `_read_header` will read 31 bytes and unpack the `chunk_size`.
- **Decoding**: `_decode_chunk` will use `self.chunk_size` instead of a hardcoded default.

## Data Flow
1. **Export**: `config.json` -> `obcm_pack.py` -> `serialize_all` -> `map.obcm` (Header contains ChunkSize).
2. **Import**: `map.obcm` -> `OBCMReader._read_header` -> `self.chunk_size`.
3. **Query**: `OBCMReader.query_bbox` -> `OBCMReader.decode_chunk` (Uses `self.chunk_size`).

## Testing & Verification
### Automated Tests
- Update `tests/test_reader.py` to use the 31-byte header in mock streams.
- Add a test case for different chunk sizes (e.g., 1024 vs 4096) and verify `decode_chunk` works for both.
- Update `tests/test_serialize.py` to verify the new header length and field.

### Manual Verification
1. Export a map with `"chunk_size": 2048` in `config.json`.
2. Open in visualizer.
3. Toggle bounding boxes ('B') and verify the grid matches the smaller/larger chunks.
