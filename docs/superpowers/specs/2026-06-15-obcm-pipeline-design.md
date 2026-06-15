# Design Spec: OBCM (Open Bike Computer Map) Pipeline

**Date:** 2026-06-15  
**Version:** 1.0  
**Status:** Approved  

## 1. Overview
The OBCM pipeline converts OpenStreetMap (OSM) `.osm.pbf` files into a custom `.obcm` binary format. This format is optimized for low-power MCUs (like the nRF5340) reading from SD cards. It features a spatial index (Quadtree) and delta-compressed geometry packed into fixed-size chunks to minimize RAM usage and SD card latency.

## 2. Requirements
- **Format:** Little-Endian (`<`)
- **Coordinates:** Microdegrees (`int32`, decimal degrees * 1e6).
- **MCU Aspect Correction:** The MCU is responsible for scaling the X-axis by `cos(latitude)` to correct for WGS84 stretching.
- **Filtering:** Only process `Ways` matching a `config.json` style mapping. Ignore `Relations`.
- **Clipping:** Features are strictly clipped to Quadtree quadrant boundaries.

## 3. Binary Layout

### 3.1 Global Header (29 bytes)
| Offset | Type | Field | Description |
|--------|------|-------|-------------|
| 0x00 | char[4] | Magic | "OBCM" |
| 0x04 | uint8 | Version | 0x01 |
| 0x05 | int32 | MinLat | Microdegrees |
| 0x09 | int32 | MinLon | Microdegrees |
| 0x0D | int32 | MaxLat | Microdegrees |
| 0x11 | int32 | MaxLon | Microdegrees |
| 0x15 | uint32 | StyleOffset | Offset to Style Dictionary (Fixed 29) |
| 0x19 | uint32 | IndexOffset | Offset to Index Block (Calculated) |

### 3.2 Style Dictionary
| Type | Field | Description |
|------|-------|-------------|
| uint8 | Count | Number of style entries |
| (repeat) | Entry | ID (u8), Z-Index (u8), RGB565 (u16), Weight (u8) |

### 3.3 Index Block (BFS Quadtree)
The Quadtree is flattened using Breadth-First Search (BFS).
- Each entry is a `uint32_t`.
- **Branch Node:** Bit 31 = `1`. Bits 0-30 = Index of the first of 4 contiguous children.
- **Leaf Node:** Bit 31 = `0`. Bits 0-30 = `Chunk ID`.
- **Empty Leaf:** Special value `0x7FFFFFFF` (all bits 1 except bit 31).

### 3.4 Data Blocks (Geometry Chunks)
Data starts at `DataStartOffset`. A chunk's location is `DataStartOffset + (ChunkID * CHUNK_SIZE)`.
- **CHUNK_SIZE:** Default 4096 bytes (configurable).
- **Padding:** Unused space in a chunk is padded with `0xFF`.

#### Feature Header (8 bytes)
| Offset | Type | Field | Description |
|--------|------|-------|-------------|
| 0x00 | uint8 | StyleID | Index into Style Dictionary |
| 0x01 | uint16 | PointCount| Number of points (including Anchor) |
| 0x03 | int16 | AnchorX | Offset from Quadrant MinLon |
| 0x05 | int16 | AnchorY | Offset from Quadrant MinLat |
| 0x07 | uint8 | DeltaFlag | 1 = 8-bit deltas, 2 = 16-bit deltas |

#### Coordinate Data
- Chained deltas (e.g., `P[i] = P[i-1] + Delta`).
- Format depends on `DeltaFlag` (`int8` or `int16`).

## 4. Implementation Strategy

### 4.1 `ingest.py`
- Uses `pyosmium` with `FlexMem` cache.
- Extracts `Ways`, converts to `shapely` geometries.
- Performs "Loose Crop" at map boundaries.

### 4.2 `quadtree.py`
- Recursive spatial partitioning.
- Leaf criteria: 
  1. Serialized size <= `CHUNK_SIZE`.
  2. Dimensions <= 32767 microdegrees.
- Clips geometries using `shapely.intersection`.
- Flattens `MultiLineStrings` into separate simple features.

### 4.3 `serialize.py`
- BFS queue for Quadtree flattening.
- Delta compression logic (deciding between 8-bit and 16-bit).
- `struct.pack` for all binary output.

### 4.4 `obcm_pack.py`
- CLI wrapper and main pipeline coordinator.

## 5. Success Criteria
- Valid `.obcm` file generated from `.pbf`.
- File passes internal validation (no overlapping chunks, valid index pointers).
- All coordinate deltas fit within their specified bit-widths.
