# OBCM File Format Specification

OBCM (OpenStreetMap Binary Chunked Map) is a compact, binary format designed for efficient rendering on memory-constrained devices, such as microcontrollers (MCUs).

## File Structure Overview

The OBCM file is structured as a contiguous binary blob, optimized for random access:

| Section | Size | Description |
| :--- | :--- | :--- |
| **Header** | 31 bytes | File identification, bounding box, and section offsets. |
| **Style Table** | Variable | Definitions of visual styles (colors, z-index, etc.). |
| **Quadtree Index** | Variable | A flat, serialized representation of the spatial quadtree. |
| **Data Chunks** | Variable | Fixed-capacity blocks containing packed feature geometry. |

---

## 1. Header (31 bytes)

The header provides the necessary metadata to parse the rest of the file.

| Field | Size | Type | Description |
| :--- | :--- | :--- | :--- |
| Magic | 4 | `char` | Must be `b"OBCM"` |
| Version | 1 | `uint8` | Format version (currently `0x02`) |
| Min Lat | 4 | `int32` | Global BBox Min Latitude (microdegrees) |
| Min Lon | 4 | `int32` | Global BBox Min Longitude (microdegrees) |
| Max Lat | 4 | `int32` | Global BBox Max Latitude (microdegrees) |
| Max Lon | 4 | `int32` | Global BBox Max Longitude (microdegrees) |
| Style Offset | 4 | `uint32` | Byte offset to Style Table |
| Index Offset | 4 | `uint32` | Byte offset to Quadtree Index |
| Chunk Size | 2 | `uint16` | Fixed capacity of each Data Chunk (bytes) |

---

## 2. Style Table

The style table maps numerical IDs to rendering properties.

**Format:**
1.  **Count** (`uint8`): Number of styles.
2.  **Style Records** (repeated `Count` times):

| Field | Size | Type | Description |
| :--- | :--- | :--- | :--- |
| ID | 1 | `uint8` | Unique Style ID |
| Z-Index | 1 | `int8` | Rendering layer order |
| Color | 2 | `uint16` | RGB565 color value |
| Weight | 1 | `uint8` | Stroke width (for lines) |

---

## 3. Quadtree Index

This is a flat array of `uint32` values representing the quadtree. The index enables spatial filtering without loading the entire map.

- **Leaf Node:** A value not containing the highest bit (`0x80000000`).
    - `0x7FFFFFFF`: Empty leaf node.
    - `Other`: `Chunk ID` of the corresponding Data Chunk.
- **Branch Node:** A value with the highest bit set (`0x80000000`).
    - Value: `0x80000000 | Index` to the first child node.
    - Children are stored sequentially: `NW`, `NE`, `SW`, `SE`.

---

## 4. Data Chunks (Fixed-Capacity)

Features are stored in fixed-capacity blocks defined by the header's `Chunk Size`. If data does not fill the chunk, it is padded with `0xFF`.

### Feature Header (12 bytes)

| Field | Size | Type | Description |
| :--- | :--- | :--- | :--- |
| Style ID | 1 | `uint8` | Link to Style Table |
| Pt Count | 2 | `uint16` | Points in exterior ring |
| Anchor X | 4 | `int32` | Chunk-relative X-offset (microdegrees) |
| Anchor Y | 4 | `int32` | Chunk-relative Y-offset (microdegrees) |
| Flags | 1 | `uint8` | Bitmask: `0x01` (16-bit deltas), `0x02` (Polygon), `0x04` (Has holes) |

### Geometry Encoding (Deltas)

Geometry is stored using **Delta Encoding** to minimize size.

1.  **Anchor Point**: The first point in a ring is stored absolutely relative to the chunk anchor.
2.  **Deltas**: Subsequent points are stored as `(dx, dy)` relative to the previous point.
3.  **Bit Depth**:
    - If `Flags & 0x01` is `0`: `dx`/`dy` are `int8`.
    - If `Flags & 0x01` is `1`: `dx`/`dy` are `int16`.

#### Polygon with Holes Example
```text
[Header]
[Exterior Deltas (int8/int16)]
[Hole Count (uint8)]
    [Hole 1 Point Count (uint16)]
    [Hole 1 Deltas (int8/int16)]
    [Hole 2 Point Count (uint16)]
    [Hole 2 Deltas (int8/int16)]
[Padding (0xFF)]
```
