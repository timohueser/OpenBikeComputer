# OBCR File Format Specification (v4)

OBCR (OpenStreetMap Binary Chunked Route) is the binary **route** format, the sibling of
the [`OBCM`](OBCM_Spec.md) map format. A route is one ordered polyline with per-point
elevation, precomputed ride statistics, an optional table of **waypoints** pinned along
the route, and an optional accepted-visit descriptor. The device and the simulator produce
it from an uploaded GPX file; the companion app produces it from imported GPX/TCX before a
BLE upload (see [`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md)).

This document is the normative byte contract. `firmware/obc-formats/src/obcr.rs` is its
code authority for versions, fixed lengths, magic, and sentinels;
`firmware/obc-formats/src/io.rs` owns the byte-source/sink traits and the checked
little-endian primitives used by both producer and reader.

**Versions.** v4 is the only accepted version. Re-import older routes from their source files.

All multi-byte integers are **little-endian**. Coordinates are **microdegrees**
(1e-6 degrees). Distances and elevations are whole **meters**. Every section is reached
through an explicit offset and every count is stored.

## File layout

```
[Header]                 (160 bytes, fixed)
[Chunk 0 data][Chunk 1 data]...[Chunk N-1 data]
[Chunk Index]            (Chunk Count × 44-byte ChunkMeta)
[Waypoints]              (optional: Waypoint Count × 80-byte records)
[Visit descriptor]       (optional: 80 bytes)
```

Every section is reached by an **explicit offset** (`Index Offset`, `Data Offset`,
`Waypoint Offset`, per-chunk `Byte Offset`), so the physical order is not load-bearing:
the reader accepts any arrangement. The canonical writer emits the index and the waypoint
table **last**, because their sizes and positions are not known until the chunks have
streamed out (§5).

---

## 1. Header (core: 112 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCR"` |
| 4 | Version | 1 | `uint8` | `0x04`; readers reject anything else |
| 5 | Flags | 1 | `uint8` | bit 0 unresolved avoidance; bit 1 at least one valid elevation; bit 2 attribution-map identity present; bit 3 Assistant candidate; other bits zero |
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
`Index Offset == Data Offset + total chunk-data bytes`. The UI derives km/m from these
meters fields (`distance_km = round(total_distance / 1000)`).

Readers validate the complete fixed header, including the optional-section envelopes.

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

An absent map key is all zero. A present key names the installed map used for surface
attribution; its object ID and revision are nonzero. A file path or a default ID does not
establish map identity. Facts from a different map revision are stale; facts without a key
are unbound. Historical surfaces can still be displayed.

A descriptor must fit inside the object and must not overlap the index or the waypoint
table. Unsupported versions and nonzero absent-section fields are errors.

An Assistant candidate is immutable. Its flag remains set after acceptance. A device can
offer it as an ordinary route only when the current card Metadata has an accepted route row
with the same object ID, revision, length, and CRC. An orphan candidate stays unavailable
after reboot. The optional Navigator checkpoint and the accepted route row are published in
the same Metadata operation. Clearing that checkpoint preserves the accepted row. Replacing
the route does not inherit the old acceptance.

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

---

## 3. Chunk data

The chunk's **first point is the anchor** (Anchor Lon/Lat/Elevation from its `ChunkMeta`)
and is **not** stored in the data. The remaining `Point Count − 1` points follow as fixed
7-byte records:

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| dLon | 2 | `int16` | Δ longitude from the previous point (microdegrees) |
| dLat | 2 | `int16` | Δ latitude from the previous point |
| Elevation | 2 | `int16` | Absolute elevation, meters; `INT16_MIN` unknown |
| Segment facts | 1 | `uint8` | bits 0..2 surface class; bit 3 incoming elevation incomplete; bits 4..7 zero |

Surface classes match OBCM: 0 unknown, 1 paved, 2 compacted, 3 gravel, 4 dirt, 5 rough,
6 cobbles, 7 grass. Each record owns the incoming segment. The shared chunk anchor has no
second incoming segment: the preceding chunk owns that segment. A trim that starts at an
anchor must retain validity for the next incoming segment.

A valid elevation is in `-32767..=32767` metres; producers clamp values to that range. Zero
is valid sea-level elevation. Missing endpoints invalidate the incoming segment. Bit 3 also
invalidates a segment with valid endpoints when graph integration found an interior terrain
gap. A missing value or an invalid segment pauses ascent; the next valid run starts a fresh
reference.

Decoding a chunk:

```
(lon, lat, ele) = (anchor_lon, anchor_lat, anchor_ele)   // first point
for each record:
    lon += dLon; lat += dLat; ele = record.elevation     // next point
```

Consecutive chunks MUST share their boundary vertex: chunk `k`'s last decoded point equals
chunk `k+1`'s anchor. `Point Count` in the header counts that vertex once.

> **Densification:** the converter inserts intermediate points on any decimated segment
> whose Δlon or Δlat would exceed the `int16` range (±32767 µdeg ≈ 3.6 km), so readers need
> no wide-delta path.

---

## 4. Waypoints

`Waypoint Count` fixed 80-byte records at `Waypoint Offset`, sorted ascending by
`Distance Along` (ties keep source order).

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

Absent provenance requires all 36 bytes at offsets 44..80 to be zero. Present provenance
requires nonzero object and revision. A transform preserves existing provenance. The first
accepted-visit producer supplies it when it has the original route's exact source identity.

**Category** reuses the map's browsable POI categories —
`1` water · `2` campsite · `3` accommodation · `4` resupply · `5` pharmacy ·
`6` bike shop ([`OBCM_Spec.md`](OBCM_Spec.md) §7.4) — and `0` (generic) is first-class.
Producers map their source symbols onto these ids; §4.1 is the canonical table for GPX.

**`Distance Along`** is defined by nearest-point placement: the cumulative route distance at
the raw track point nearest the waypoint's coordinate.

**`Lateral Offset`** comes out of that same placement. Its **magnitude** is the ground
distance from the waypoint to the track point that won it. Its **sign** is which side of the
local direction of travel the waypoint fell on — negative left, positive right (the cross
product of the travel vector with the offset vector). A waypoint exactly on the line of
travel takes the positive sign; one *on* a track vertex is `0`.

### 4.1 GPX symbol → category (canonical)

A GPX `<wpt>` names its icon in `<sym>` (Garmin's symbol names) or `<type>` (RideWithGPS'
and Komoot's POI class). The producer takes **`<sym>` if non-empty, else `<type>`**, matches
it **case- and separator-insensitively** (`Drinking Water` = `drinking_water` =
`drinking-water`), and stores the category below. Anything unmapped stores `0` (generic): a
waypoint is **never dropped** for its symbol.

| Category | Symbols |
| :-- | :-- |
| `1` water | water · drinking water · water source · water point · potable water · fountain · drinking fountain · spring · water tap · tap · well |
| `2` campsite | campground · camping · campsite · camp site · camp · tent · caravan site · rv park |
| `3` accommodation | lodging · hotel · hostel · motel · inn · guest house · guesthouse · bed and breakfast · b&b · accommodation · cabin · hut · alpine hut · wilderness hut · refuge |
| `4` resupply | resupply · convenience store · convenience · grocery · grocery store · supermarket · shopping center · shopping · store · market · marketplace · bakery · food · restaurant · fast food · pizza · diner · cafe · coffee · bar · pub · gas station · fuel |
| `5` pharmacy | pharmacy · chemist · drugstore · apothecary |
| `6` bike shop | bike shop · bicycle shop · bike store · cycle shop · cyclery · bike repair · bicycle repair · bike service |

Symbols with no home among the six stay generic: "Restroom", "Parking", "Ferry",
"Hospital", "First Aid", "Viewpoint" and "Summit" are deliberately absent. Eating and
shopping share **resupply**; there is no separate food category.

`firmware/obc-route/src/symbol.rs` is the code mirror of this table, row for row.

---

## 5. Conversion

- The emitter measures the final retained geometry.
- Segment distance uses `obc_map_scene::ground_dist_m`: microdegree coordinates, `f32` local
  equirectangular segment math, accumulated in `f64`. Stored totals and cumulative anchors
  truncate to whole metres.
- Ascent and descent use a 3 m elevation dead-band. A segment that crosses no integer
  distance boundary books no ascent or descent. Missing samples and bit 3 pause the
  reference. Min/max use valid retained elevations only; both are zero if none exist.
- Integer-prefix clipping divides each segment's booked ascent and descent over its integer
  distance span. Adjacent intervals conserve distance, surface, ascent, and descent exactly.
  Grades use ordered endpoint rise over physical distance.
- Plain imports store unknown surface. Map-assisted imports require a unique graph edge at
  both endpoints and the midpoint, a consistent direction and along-edge distance, and a
  bounded distance from the graph; parallel roads, repeated crossings, ambiguous junctions,
  and long sparse spans stay unknown.
- The decimator retains elevation changes, reversals, surface transitions, and validity
  boundaries. Coordinate deltas are split before they exceed `int16`.
- Geometry uses at most 256 points per chunk and 256 chunks. Over-cap routes fail. The GPX
  importer collects at most 32 waypoints. When waypoints exist, a separate track pass selects
  their nearest raw vertices; the emit pass retains these anchors and records their distance
  on the final stored geometry.
- Writers stream a header placeholder, the chunks, the index, and the waypoints, then patch
  the header. Transform producers retain surfaces, validity, waypoint provenance, and
  unresolved-avoidance state. A shared map key survives only when both sources agree.

## 6. Accepted visit descriptor

Schema 1 is exactly 80 bytes. Anchors are cumulative metres on the named geometry. The
original route remains a separate immutable object.

| Offset | Field | Bytes |
| :-- | :-- | :-- |
| 0 | Original Store ID, Object ID, revision | 16 + 8 + 8 |
| 32 | Original entry, stop, rejoin anchors | 3 × `uint32` |
| 44 | Accepted entry, stop, rejoin anchors | 3 × `uint32` |
| 56 | Target source ID | `uint64` |
| 64 | Target longitude, latitude, microdegrees | 2 × `int32` |
| 72 | Target source kind: 1 OSM node, 2 way, 3 relation, 4 Wikidata Q ID | `uint8` |
| 73 | Reserved, zero | 7 |

Each anchor triple is monotonic. Accepted rejoin cannot exceed route length. Coordinates are
within geographic bounds; target ID, original object ID, and revision are nonzero. Unknown
target kinds are errors. This record describes the accepted route; navigation phase and the
durable checkpoint CRC/length are separate.

---

## Reference implementation

`firmware/obc-formats` (`no_std`): `obcr.rs` (normative version, sizes, magic, and
sentinels) and `io.rs` ([`ByteSource`](#bytesource)/`ByteSink` + endian primitives).
`firmware/obc-route` (`no_std`): `reader.rs` (`RouteReader`, `RouteSummary`, `ChunkMeta`,
`Waypoint` + `for_each_waypoint`), `convert.rs` (GPX → OBCR), `gpx.rs` (streaming `<trkpt>`
+ `<wpt>` scans), and `symbol.rs` (§4.1's table). Format-contract tests build synthetic
`.obcr` bytes by hand, mirroring this layout (`obc-route/tests/format.rs` +
`tests/waypoints.rs`); shared phone↔firmware fixtures live in `specs/vectors/`.
`route-visit.obcr` has a valid descriptor envelope. Both codecs MUST reject
`route-visit-waypoint-overlap.obcr` and `route-visit-index-overlap.obcr`; each shares the
descriptor's last four reserved-zero bytes with another section.

### ByteSource

```rust
pub trait ByteSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
    fn len(&self) -> u64;
}
```

The shared borrow lets the host implementation copy from a `&[u8]` and the device implementation
read a held flat-store object. `RouteReader` holds `&dyn ByteSource`, so it stays monomorphic.
