# OBCR File Format Specification (v4)

OBCR (OpenStreetMap Binary Chunked Route) is a compact binary **route** format —
the route-planning sibling of the [`OBCM`](OBCM_Spec.md) map format. A route is a
single ordered polyline with per-point elevation, plus precomputed ride statistics
and an optional table of **waypoints** pinned along the route. It is
produced **on the device** (or in the simulator) by converting an uploaded GPX
file — or **on the phone** by the companion app, which encodes imported GPX/TCX
to OBCR before a BLE upload (see
[`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md)) — and read back by the
same `no_std` Rust code that the firmware runs (`firmware/obc-route`).

This document is the normative byte contract. `firmware/obc-formats/src/obcr.rs`
is its code authority for versions, fixed lengths, magic, and sentinels;
`firmware/obc-formats/src/io.rs` owns the neutral byte-source/sink traits and checked
little-endian primitives used by both producer and reader.

**Versions.** v4 is the only accepted version. Re-import older routes from their source files.

The waypoints section is still reached only via an explicit offset, so a reader
that doesn't care about waypoints skips it in O(1) by construction and the ride
path never touches a waypoint byte.

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
   are computed from retained geometry by the shared emitter. Interval facts use
   the same points, distance metric, and elevation validity.
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
[Header]                 (160 bytes, fixed)
[Chunk 0 data][Chunk 1 data]...[Chunk N-1 data]
[Chunk Index]            (Chunk Count × 44-byte ChunkMeta)
[Waypoints]              (optional: Waypoint Count × 80-byte records)
[Visit descriptor]       (optional: 80 bytes)
```

Every section is reached by an **explicit offset** (`Index Offset`, `Data Offset`,
`Waypoint Offset`, per-chunk `Byte Offset`), so the physical order is not
load-bearing — the reader accepts any arrangement. The canonical writer emits the
index and waypoint table **last** because their sizes/positions aren't known until
the chunks have streamed out (see §5).

---

## 1. Header (core: 112 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCR"` |
| 4 | Version | 1 | `uint8` | `0x04`; readers reject anything else |
| 5 | Flags | 1 | `uint8` | bit 0 unresolved avoidance; bit 1 at least one valid elevation; bit 2 attribution-map identity present; other bits zero |
| 6 | Name Len | 1 | `uint8` | Used bytes of the Name field (≤ 48) |
| 7 | Reserved | 1 | `uint8` | `0` |
| 8 | Min Lon | 4 | `int32` | Global bbox, microdegrees |
| 12 | Min Lat | 4 | `int32` | |
| 16 | Max Lon | 4 | `int32` | |
| 20 | Max Lat | 4 | `int32` | |
| 24 | Start Lon | 4 | `int32` | First route point (camera centering) |
| 28 | Start Lat | 4 | `int32` | |
| 32 | Point Count | 4 | `uint32` | Distinct stored points (seams counted once) |
| 36 | Total Distance | 4 | `uint32` | Meters, measured from retained geometry |
| 40 | Total Ascent | 4 | `uint32` | Meters, dead-banded retained geometry |
| 44 | Total Descent | 4 | `uint32` | Meters, smoothed |
| 48 | Min Elevation | 2 | `int16` | Meters |
| 50 | Max Elevation | 2 | `int16` | Meters |
| 52 | Chunk Count | 4 | `uint32` | Number of geometry chunks (≥ 1) |
| 56 | Index Offset | 4 | `uint32` | Byte offset to the Chunk Index |
| 60 | Data Offset | 4 | `uint32` | Byte offset to Chunk 0 data |
| 64 | Name | 48 | `char[48]` | UTF-8 route name, null-padded |

With the canonical layout, `Data Offset == 160` (chunks follow the header) and
`Index Offset == Data Offset + total chunk-data bytes`. Distance/ascent in **km/m**
for the UI are derived from these meters fields (`distance_km = round(total_distance /
1000)`).

Readers validate the complete fixed header, including optional-section envelopes.

### 1.1 Header extension (48 bytes, at offset 112)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 112 | Waypoint Offset | 4 | `uint32` | Byte offset to the waypoints section; `0` when Waypoint Count is 0 |
| 116 | Waypoint Count | 2 | `uint16` | Stored waypoint records |
| 118 | Visit version | 1 | `uint8` | 0 absent; 1 descriptor schema below; other values rejected |
| 119 | Reserved | 1 | `uint8` | 0 |
| 120 | Visit offset | 4 | `uint32` | Absolute descriptor offset; 0 when absent |
| 124 | Visit length | 4 | `uint32` | Exactly 80 when present; 0 when absent |
| 128 | Attribution-map key | 32 | bytes | Store ID (16), object ID (`uint64`), revision (`uint64`) |

An absent map key is all zero. A present key names the installed map used for
surface attribution. Object ID and revision are nonzero. A file path or default
ID does not establish map identity. Facts from a different map revision are stale;
facts without a key are unbound. Historical surfaces can still be displayed.

A descriptor must fit inside the object and must not overlap the index or waypoint
table. Unsupported versions and nonzero absent-section fields are errors.

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
follow as fixed 7-byte records:

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| dLon | 2 | `int16` | Δ longitude from the previous point (microdegrees) |
| dLat | 2 | `int16` | Δ latitude from the previous point |
| Elevation | 2 | `int16` | Absolute elevation, meters; `INT16_MIN` unknown |
| Segment facts | 1 | `uint8` | bits 0..2 surface class; bit 3 incoming elevation incomplete; bits 4..7 zero |

Surface classes match OBCM: 0 unknown, 1 paved, 2 compacted, 3 gravel, 4 dirt,
5 rough, 6 cobbles, 7 grass. Each record owns the incoming segment. The shared
chunk anchor has no second incoming segment: the preceding chunk owns that segment.
A trim that starts at an anchor must retain validity for the next incoming segment.

A valid elevation is in `-32767..=32767` metres; producers clamp values to that
range. Zero is valid sea-level elevation. Missing endpoints invalidate the incoming
segment. Bit 3 also invalidates a segment with valid endpoints when graph integration
found an interior terrain gap. A missing value or invalid segment pauses ascent;
the next valid run starts a fresh reference.

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

## 4. Waypoints

`Waypoint Count` fixed 80-byte records at `Waypoint Offset`, sorted ascending by
`Distance Along` (ties keep source order). A point of interest pinned to a position
along the route: what the rider planned around, carried beside the map's own POIs in
one route-ordered list.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Distance Along | 4 | `uint32` | Meters from the route start to the waypoint's position on the track |
| 4 | Lon | 4 | `int32` | The waypoint's own coordinate, microdegrees (may sit off the polyline) |
| 8 | Lat | 4 | `int32` | |
| 12 | Elevation | 2 | `int16` | Meters; `INT16_MIN` (−32768) = unknown |
| 14 | Category | 1 | `uint8` | `0` = generic, `1..=6` = the OBCM §7.4 category ids; render any other value as generic |
| 15 | Name Len | 1 | `uint8` | Used bytes of Name (≤ 24) |
| 16 | Lateral Offset | 2 | `int16` | Meters off the route line, **positive = right** of the direction of travel; `0` = on-route. Saturating |
| 18 | Reserved | 2 | — | `0` |
| 20 | Name | 24 | `char[24]` | UTF-8 short name, null-padded |
| 44 | Original store | 16 | bytes | Original route Store ID |
| 60 | Original object | 8 | `uint64` | Original route Object ID |
| 68 | Original revision | 8 | `uint64` | Original route revision |
| 76 | Original ordinal | 2 | `uint16` | Original waypoint ordinal |
| 78 | Provenance flags | 2 | `uint16` | 1 present; 0 absent; other values rejected |

Absent provenance requires all 36 bytes at offsets 44..80 to be zero. Present
provenance requires nonzero object and revision. A transform preserves existing
provenance. The first accepted visit producer supplies it when it has the original
route's exact source identity.

**Category** reuses the map's browsable POI categories verbatim —
`1` water · `2` campsite · `3` accommodation · `4` resupply · `5` pharmacy ·
`6` bike shop ([`OBCM_Spec.md`](OBCM_Spec.md) §7.4) — so a stored waypoint and a
map POI share one icon language, and `0` (generic) is first-class: most
hand-placed waypoints ("turn left here") map to nothing and render as a plain
diamond. Producers map their source symbols onto these ids; §4.1 is the canonical
table for GPX.

`Distance Along` is defined by nearest-point placement: the cumulative route
distance at the raw track point nearest the waypoint's coordinate (how both the
phone importer and the firmware converter place free-standing GPX `<wpt>`s, which
carry no ride-order of their own).

`Lateral Offset` comes out of that same placement: its **magnitude** is the ground
distance from the waypoint to the track point that won it, and its **sign** is
which side of the local direction of travel the waypoint fell on — negative left,
positive right (the cross product of the travel vector with the offset vector; a
waypoint exactly on the line of travel takes the positive sign, and one *on* a
track vertex is simply `0`). It is stored, not derived at read time, because a
riding device has no cheap way to re-measure it: the answer needs the raw track
the converter saw, not the decimated geometry it stored.

### 4.1 GPX symbol → category (canonical)

A GPX `<wpt>` names its icon in `<sym>` (Garmin's symbol names, which most planners
copy) or `<type>` (RideWithGPS' and Komoot's POI class). Neither is a registry, so
this table is a **curation** from real Komoot / RideWithGPS / Garmin BaseCamp
exports. The producer takes **`<sym>` if non-empty, else `<type>`**, matches it
**case- and separator-insensitively** (`Drinking Water` = `drinking_water` =
`drinking-water`), and stores the category below. Anything unmapped stores `0`
(generic) — a waypoint is **never dropped** for its symbol.

| Category | Symbols |
| :-- | :-- |
| `1` water | water · drinking water · water source · water point · potable water · fountain · drinking fountain · spring · water tap · tap · well |
| `2` campsite | campground · camping · campsite · camp site · camp · tent · caravan site · rv park |
| `3` accommodation | lodging · hotel · hostel · motel · inn · guest house · guesthouse · bed and breakfast · b&b · accommodation · cabin · hut · alpine hut · wilderness hut · refuge |
| `4` resupply | resupply · convenience store · convenience · grocery · grocery store · supermarket · shopping center · shopping · store · market · marketplace · bakery · food · restaurant · fast food · pizza · diner · cafe · coffee · bar · pub · gas station · fuel |
| `5` pharmacy | pharmacy · chemist · drugstore · apothecary |
| `6` bike shop | bike shop · bicycle shop · bike store · cycle shop · cyclery · bike repair · bicycle repair · bike service |

Symbols with no honest home among the six stay generic rather than being forced into
the nearest one — "Restroom", "Parking", "Ferry", "Hospital", "First Aid",
"Viewpoint" and "Summit" are all deliberately absent. Eating and shopping share
**resupply**: there is no separate food category, and a rider looking for supplies
wants the bakery and the café in one list.

`firmware/obc-route/src/symbol.rs` is the code mirror of this table, row for row.

---

## 5. Conversion and measured facts

- The emitter measures the final retained geometry. Search costs never replace route distance.
- Segment distance uses `obc_map_scene::ground_dist_m`: microdegree coordinates,
  `f32` local equirectangular segment math, accumulated in `f64`. Stored totals and
  cumulative anchors truncate to whole metres.
- Facts policy 1 uses a 3 m elevation dead-band. A segment that crosses no integer
  distance boundary books no ascent or descent. Missing samples and bit 3 pause
  the reference. Min/max use valid retained elevations only; both are zero if none exist.
- Integer-prefix clipping divides each segment's booked ascent and descent over
  its integer distance span. Adjacent intervals conserve distance, surface, ascent,
  and descent exactly. Grades use ordered endpoint rise over physical distance.
- GPX completeness means complete samples of the imported geometry. It makes no
  claim about unsampled terrain. Plain imports store unknown surface. Map-assisted
  imports require a unique graph edge at both endpoints and the midpoint, consistent
  direction and along-edge distance, and bounded distance from the graph. Parallel
  roads, repeated crossings, ambiguous junctions, and long sparse spans stay unknown.
- The decimator retains elevation changes, reversals, surface transitions, and
  validity boundaries. Coordinate deltas are split before they exceed `int16`.
- Geometry uses at most 256 points per chunk and 256 chunks. Over-cap routes fail.
  The GPX importer collects at most 32 waypoints. If waypoints exist, a separate
  track pass selects their nearest raw vertices; the emit pass retains these anchors
  and records their distance on the final stored geometry. No route-length array is added.
- Writers stream header placeholder, chunks, index, and waypoints, then patch the
  header. Transform producers retain surfaces, validity, waypoint provenance, and
  unresolved-avoidance state. A shared map key survives only when both sources agree.

## 6. Accepted visit descriptor

Schema 1 is exactly 80 bytes. Anchors are cumulative metres on the named geometry.
The original route remains a separate immutable object.

| Offset | Field | Bytes |
| :-- | :-- | :-- |
| 0 | Original Store ID, Object ID, revision | 16 + 8 + 8 |
| 32 | Original entry, stop, rejoin anchors | 3 × `uint32` |
| 44 | Accepted entry, stop, rejoin anchors | 3 × `uint32` |
| 56 | Target source ID | `uint64` |
| 64 | Target longitude, latitude, microdegrees | 2 × `int32` |
| 72 | Target source kind: 1 OSM node, 2 way, 3 relation, 4 Wikidata Q ID | `uint8` |
| 73 | Reserved, zero | 7 |

Each anchor triple is monotonic. Accepted rejoin cannot exceed route length.
Coordinates are within geographic bounds; target ID, original object ID, and
revision are nonzero. Unknown target kinds are errors. This record describes the
accepted route; navigation phase and durable checkpoint CRC/length are separate.

## 7. Producer and consumer matrix

| Path | Surface and elevation | Optional metadata |
| :-- | :-- | :-- |
| Rust GPX / browser converter | Unknown surface by default; explicit missing elevation | No visit descriptor; no original identity invented |
| Simulator GPX import with mounted map | Conservative graph attribution and exact installed map key | Same as GPX |
| Rust navigation emitter | Graph surface; sampled heights; graph interior-gap validity | Optional map key supplied by owner |
| Rust trim / detour splice | Preserve incoming facts; measure final geometry | Preserve waypoint provenance and unresolved avoidance; reject visit-bearing inputs until visit transform is supplied |
| Rust reader / interval API / profiles | v4 only; bounded chunk scratch; gaps remain incomplete | Validate source key, provenance, descriptor |
| Swift encoder / library / reversal | v4; preserve surfaces and missing spans; reverse incoming ownership | Preserve waypoint provenance; fresh imports have no map key or visit descriptor |
| Swift decoder | v4; decode surfaces and gaps | Decode and validate map key and descriptor; no visit producer |
| Catalog summaries and BLE transfer | Read v4 header; transfer immutable bytes | Do not rewrite payload metadata |
| Shared vectors / authored fixture converter | Production Rust writer | Checked by Rust and Swift readers |

---

## Reference implementation

`firmware/obc-formats` (`no_std`): `obcr.rs` (normative version, sizes, magic, and
sentinels) and `io.rs` ([`ByteSource`](#bytesource)/`ByteSink` + endian primitives).
`firmware/obc-route` (`no_std`): `reader.rs` (`RouteReader`, `RouteSummary`,
`ChunkMeta`, `Waypoint` + `for_each_waypoint`), `convert.rs` (GPX → OBCR),
`gpx.rs` (streaming `<trkpt>` + `<wpt>` scans), and `symbol.rs` (§4.1's table).
Format-contract tests build synthetic
`.obcr` bytes by hand, mirroring this layout (`obc-route/tests/format.rs` +
`tests/waypoints.rs`); shared phone↔firmware fixtures live in `specs/vectors/`.
`route-visit.obcr` has a valid descriptor envelope. Both codecs MUST reject
`route-visit-waypoint-overlap.obcr` and `route-visit-index-overlap.obcr`; each
shares the descriptor's last four reserved-zero bytes with another section.

### ByteSource

```rust
pub trait ByteSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
    fn len(&self) -> u64;
}
```

`&self` (shared borrow): the host impl copies from a `&[u8]`; the device impl wraps a
FatFs file with interior mutability. `RouteReader` holds `&dyn ByteSource`, so it stays
monomorphic and the genericity never reaches the screen stack.
