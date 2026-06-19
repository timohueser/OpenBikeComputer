# OBCR File Format Specification (v1)

OBCR (OpenStreetMap Binary Chunked Route) is a compact binary **route** format —
the route-planning sibling of the [`OBCM`](OBCM_Spec.md) map format. A route is a
single ordered polyline with per-point elevation, plus precomputed ride statistics.
It is produced **on the device** (or in the simulator) by converting an uploaded GPX
file, and read back by the same `no_std` Rust code that the firmware runs
(`firmware/obc-route`).

It shares OBCM's conventions so the reader/renderer feel identical: little-endian
integers, coordinates in **microdegrees** (1e-6 degrees), per-chunk **anchor +
delta-encoded** geometry, and **no runtime discovery** (every section is reached via
an explicit offset and every count is stored).

## Design principles

1. **Chunked + streamable.** Geometry is split into fixed-capacity chunks indexed by
   a small resident table. The reader loads the header + index into RAM and pulls
   individual chunks **on demand** through a [`ByteSource`](#bytesource) — a
   hundreds-of-km route never has to be RAM-resident. This is the 1-D analog of
   OBCM's quadtree: a route is a path, so the index is a flat list scanned linearly
   (chunk counts are small).
2. **Stats precomputed, exact.** Total distance / ascent / descent / elevation range
   are computed at conversion from **all** raw GPX points and stored in the header.
   The displayed totals are therefore exact even though the stored geometry is
   decimated for drawing.
3. **Convert where it lands.** The GPX→OBCR converter is one portable `no_std`
   routine; the device runs it on a USB/BLE upload, the simulator runs it on import.
   There is no off-device conversion step.
4. **Seam-sharing chunks.** Consecutive chunks **share their boundary vertex** (chunk
   `k`'s last point == chunk `k+1`'s anchor). A renderer can therefore draw each
   chunk's polyline independently with no gap at the seam, and cumulative stats join
   continuously across chunks.

All multi-byte integers are **little-endian**. Distances/elevations are whole
**meters**.

## File layout

```
[Header]                 (112 bytes, fixed)
[Chunk 0 data][Chunk 1 data]...[Chunk N-1 data]
[Chunk Index]            (Chunk Count × 44-byte ChunkMeta)
```

Every section is reached by an **explicit offset** (`Index Offset`, `Data Offset`,
per-chunk `Byte Offset`), so the physical order is not load-bearing — the reader
accepts either arrangement. The canonical writer emits the index **last** because its
size isn't known until the chunks have streamed out (see §4).

---

## 1. Header (112 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCR"` |
| 4 | Version | 1 | `uint8` | `0x01` |
| 5 | Flags | 1 | `uint8` | Reserved, `0` |
| 6 | Name Len | 1 | `uint8` | Used bytes of the Name field (≤ 48) |
| 7 | Reserved | 1 | `uint8` | `0` |
| 8 | Min Lon | 4 | `int32` | Global bbox, microdegrees |
| 12 | Min Lat | 4 | `int32` | |
| 16 | Max Lon | 4 | `int32` | |
| 20 | Max Lat | 4 | `int32` | |
| 24 | Start Lon | 4 | `int32` | First route point (camera centering) |
| 28 | Start Lat | 4 | `int32` | |
| 32 | Point Count | 4 | `uint32` | Distinct stored points (seams counted once) |
| 36 | Total Distance | 4 | `uint32` | Meters, exact (from raw GPX) |
| 40 | Total Ascent | 4 | `uint32` | Meters, smoothed (from raw GPX) |
| 44 | Total Descent | 4 | `uint32` | Meters, smoothed |
| 48 | Min Elevation | 2 | `int16` | Meters |
| 50 | Max Elevation | 2 | `int16` | Meters |
| 52 | Chunk Count | 4 | `uint32` | Number of geometry chunks (≥ 1) |
| 56 | Index Offset | 4 | `uint32` | Byte offset to the Chunk Index (== 112) |
| 60 | Data Offset | 4 | `uint32` | Byte offset to Chunk 0 data |
| 64 | Name | 48 | `char[48]` | UTF-8 route name, null-padded |

With the canonical (index-last) layout, `Data Offset == 112` (chunks follow the header)
and `Index Offset == Data Offset + total chunk-data bytes`. Distance/ascent in **km/m**
for the UI are derived from these meters fields (`distance_km = round(total_distance /
1000)`).

---

## 2. Chunk Index

`Chunk Count` entries, in route order. Each is 44 bytes (`ChunkMeta`):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Min Lon | 4 | `int32` | Chunk bbox (viewport query), microdegrees |
| 4 | Min Lat | 4 | `int32` | |
| 8 | Max Lon | 4 | `int32` | |
| 12 | Max Lat | 4 | `int32` | |
| 16 | Anchor Lon | 4 | `int32` | Absolute coord of the chunk's first point |
| 20 | Anchor Lat | 4 | `int32` | |
| 24 | Anchor Elevation | 2 | `int16` | Meters, the first point's elevation |
| 26 | Point Count | 2 | `uint16` | Points in this chunk, **including** the anchor |
| 28 | Cum Distance | 4 | `uint32` | Meters from route start to this chunk's first point |
| 32 | Cum Ascent | 4 | `uint32` | Meters of ascent to this chunk's first point |
| 36 | Byte Offset | 4 | `uint32` | Absolute file offset to this chunk's data |
| 40 | Byte Len | 4 | `uint32` | Length of this chunk's data, bytes |

`Cum Distance` / `Cum Ascent` make "remaining distance/climb from the current
position" an O(1) subtraction once the active segment is known (Milestone 2
map-matching). The bbox enables the linear viewport query for drawing.

---

## 3. Chunk data

The chunk's **first point is the anchor** (Anchor Lon/Lat/Elevation from its
`ChunkMeta`) and is **not** stored in the data. The remaining `Point Count − 1` points
follow as fixed 6-byte records:

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| dLon | 2 | `int16` | Δ longitude from the previous point (microdegrees) |
| dLat | 2 | `int16` | Δ latitude from the previous point |
| Elevation | 2 | `int16` | **Absolute** elevation, meters |

Decoding a chunk:

```
(lon, lat, ele) = (anchor_lon, anchor_lat, anchor_ele)   // first point
for each record:
    lon += dLon; lat += dLat; ele = record.elevation     // next point
```

Position chains by delta (compact); elevation is stored absolute (simple decode, same
2 bytes). The chunk's **last** decoded point equals the next chunk's anchor (seam
sharing, §Design principle 4).

> **Densification:** the converter inserts intermediate points on any decimated
> segment whose Δlon or Δlat would exceed the `int16` range (±32767 µdeg ≈ 3.6 km), so
> readers need no wide-delta path. (Mirrors OBCM's long-segment densification.)

---

## 4. Conversion semantics (`gpx → .obcr`)

- **Single streaming pass** over the GPX `<trkpt lat lon><ele>` points (O(1) RAM,
  any route length). Distance accumulates via incremental haversine; ascent/descent
  accumulate from a smoothed elevation series with a small gain threshold (≈3 m) so
  totals read like a planner's.
- **Geometry is decimated** for storage (perpendicular-distance + max segment span)
  while **stats use every raw point** — the header totals stay exact.
- **Chunking:** points are grouped into chunks of ≤ `MAX_POINTS_PER_CHUNK` (256),
  each emitting a `ChunkMeta`. The converter raises the per-chunk point budget if a
  route would exceed `MAX_ROUTE_CHUNKS` (512), so the resident index stays bounded.
- **Writer order (device-safe, streamed):** write a placeholder header → stream chunk
  data while collecting `ChunkMeta` in a bounded in-RAM index → write the index →
  `seek(0)` and patch the header (offsets, counts, totals). Chunk bytes are never all
  resident at once.

---

## Reference implementation

`firmware/obc-route` (`no_std`): `byte_io.rs` ([`ByteSource`](#bytesource)/`ByteSink`),
`reader.rs` (`RouteReader`, `RouteSummary`, `ChunkMeta`), `convert.rs` (GPX → OBCR),
`gpx.rs` (streaming `<trkpt>` scan). Format-contract tests build synthetic `.obcr`
bytes by hand, mirroring this layout (cf. `obc-reader/tests/format.rs`).

### ByteSource

```rust
pub trait ByteSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error>;
    fn len(&self) -> u32;
}
```

`&self` (shared borrow): the host impl copies from a `&[u8]`; the device impl wraps a
FatFs file with interior mutability. `RouteReader` holds `&dyn ByteSource`, so it stays
monomorphic and the genericity never reaches the screen stack.
