# OBCM File Format Specification (v14)

OBCM (OpenStreetMap Binary Chunked Map) is a compact binary map format designed
for efficient rendering on memory-constrained devices such as microcontrollers
(MCUs). It is written by the Rust packer (`host/obc-pack`) and read by the
Rust crate (`firmware/obc-reader`, shared by the desktop simulator and the nRF54L
firmware).

This document is the normative byte contract. Its code authority for version numbers,
fixed lengths, flags, sentinels, the canonical POI id table, and endian primitives is
`firmware/obc-formats/src/obcm.rs`; producers and consumers import those facts directly.

**Version 3** introduced a **level-of-detail (LOD) pyramid**: a file holds N
self-contained detail levels, each its own quadtree + chunk set with geometry
simplified to that level's resolution. The renderer reads only the level that
matches the current zoom, so zooming out touches a small coarse layer instead of
decoding fine geometry just to skip it.

**Version 4** appends a single 2-byte field to the header — the **user-position
marker color** (RGB565).

**Version 5** adds a 6th byte to style records for flags (bit 0 = priority).

**Version 6** appends a 4-byte **POI Section Offset** to the header (32 → 36
bytes) and a new **POI section** (§7): the OSM points-of-interest the packer
bakes in (water, campsites, accommodation, resupply, pharmacies, bike shops),
indexed by a small quadtree per category over 32-byte records. The section is
**always present** — a map with no POIs writes an empty directory, never a
sentinel-zero offset.

**Version 7** widens the POI record 32 → 36 bytes: the `Name` field grows 20 → 24
bytes and the two trailing reserved bytes become a `HoursRef` u16 into a new
**hours-pool section** (§7.5). The POI directory (§7.1) gains
`hours_pool_offset` + `hours_pool_count`. The pool holds deduplicated 29-byte
weekly-schedule blobs (today's opening hours, normalized at pack time from OSM
`opening_hours`), so a POI's hours are a single index lookup on-device.

**Version 8** appends a 4-byte **Nav Graph Offset** to the header (36 → 40 bytes)
and a new **navigation-graph section** (§8) at the file tail: the routable graph
the packer derives from OSM `highway=*` topology (junction nodes with inline
adjacency, indexed by a §4-style quadtree, plus a chunked edge-geometry pool), so
the device can run point-to-point A\* (epic #116) with only a small directory
resident. The section is **always present** — a map with no routable ways writes
an empty directory, never a sentinel-zero offset.

**Version 10** (epic #556 #557) grows the **style record** 6 → 8 bytes (§2): the
flags byte gains a **dashed** bit (bit 2, line style) and a **color2-present** bit
(bit 3), and a trailing **`color2`** u16 (RGB565 secondary color) is appended. The
header, geometry, POI, and nav sections are byte-identical to v9. `color2` is written
`0x0000` when its flag bit is clear, and readers MUST ignore it then (`0x0000` is a
legit color — black rails — not a "no color2" sentinel). (v10 was the only supported
version until v11, below; the interesting detail for v11 is that the §8 padding
lesson of v9 turned out to apply to the geometry chunks too.)

**Version 11** (issue #1009) stops paying for padding. Two changes, both to the
per-LOD geometry region; the header, POI section (§7), hours pool (§7.5) and
nav-graph section (§8) are byte-identical to v10.

1. **Data chunks are packed tight, behind a per-chunk offset table** (§5). v10
   padded every chunk to `Chunk Size` because `data_start + k * Chunk Size` was the
   O(1) addressing scheme — measured **53% of `freiburg.obcm` and 65% of
   `grimsel.obcm` was trailing `0xFF`**, structurally: a quadtree node splits when
   its features overflow one chunk, so leaves land between a quarter and half full.
   A LOD now writes `Chunk Count + 1` `uint32` offsets between its index and its
   chunk data; chunk `k` is `offsets[k]..offsets[k+1]`, still O(1), and each chunk
   carries exactly **one** trailing `0xFF` sentinel instead of padding. `Chunk Size`
   keeps its 18-byte LOD-table slot but changes meaning: it is now the chunk
   **capacity bound**, not a stride.
2. **The feature header shrinks 12 → 7 bytes for the common case** (§5). `Flags`
   moves to byte 1 so its new `0x08` **WIDE** bit tells a reader the header's width
   before it reads anything behind it. The compact layout stores `Pt Count` as a
   `uint8` and both anchors as `uint16`; a feature with more than 255 exterior
   vertices, or a leaf-relative anchor outside `0..=65535` (a coarse-LOD leaf can
   span far more than that), sets WIDE and keeps v10's `uint16` count + `int32`
   anchors.

Stacked, real maps land at **~2.3–2.5× smaller** (monaco 1 597 945 → 683 532 B;
grimsel 6 189 979 → 2 614 924 B) with byte-for-byte the same decoded geometry.
Tight chunks are also a read win: a chunk miss reads the chunk's real length —
averaging ~1 600 B, 3–4 SD blocks — instead of a fixed 4096 B / 8 blocks.
(v11 was the only supported version until v12, below; its geometry sections are
unchanged by it.)

**Version 12** (issue #1073, elevation epic #1068) makes the routable graph
**climb-aware**. Two fields, one section — §8. The header, style table, geometry
(§5), POI section (§7), hours pool (§7.5), nav directory (§8.1), node quadtree
(§8.2) and edge pool (§8.4) are **byte-identical to v11**.

1. **The §8.3 neighbor entry grows 15 → 17 bytes**: a trailing `uint16 Ascent M`,
   the **integrated** climb of riding that edge *from this record's node toward
   the neighbor*, in metres, saturating. Integrated rather than an endpoint
   difference, because a pass between two equal-height junctions has hundreds of
   metres of climb and no net change — the number A\* needs is the integral. It
   lives in the adjacency entry and nowhere else because relaxation reads exactly
   that record; §8's "no second fetch" intent is the whole reason the entry
   carries its neighbor's coordinate inline.
2. **The §8.6 profile record grows 52 → 56 bytes**: a `uint8 Climb Weight` (flat
   metres charged per metre of ascent; `0` = climb-blind) plus three reserved
   bytes written `0`.

The degree cap survives untouched: `13 + 17 × 24 = 421 ≤ 512`, so a cap-degree
junction record still fits one pinned nav chunk. Real maps grow ~0.3–0.6 %.

A map packed **without** terrain writes `Ascent M = 0` everywhere and is
decode-valid: it routes exactly as v11 did — the degrade path, and what the
smaller fixtures (`monaco.obcm`, `grimsel-demo.obcm`) still carry.
`grimsel.obcm` is packed **with** its terrain sidecar since 2026-08-03 (#1096
follow-up), so it exercises real integrated ascent and the traced contours.

**Version 13** adds a sparse exact-edge lookup index to §8 so routing can recover when the rider is
close to a road but farther than 250 m from every graph junction. Only final serialized edge pieces
longer than 300 m receive interior anchors, evenly spaced so every endpoint/anchor gap is at most
300 m. Each 12-byte anchor stores an absolute coordinate plus its edge-pool id; it is a lookup aid,
not a graph node and not the snapped position. The router projects the rider onto the named full
§8.4 polyline and connects that exact point virtually to the edge's real endpoints. The §8.1
directory grows 28 → 40 bytes to address the new quadtree and fixed 512-byte chunks (§8.7). All
other records retain their v12 layouts.

The coverage bound is geometric: a point on a road is at most 150 m along the polyline from an edge
endpoint or anchor. A rider at most 100 m from that road point is therefore at most 250 m from one
lookup record by the triangle inequality, regardless of curvature. The reference router uses a
251 m node-or-anchor search (the mathematical 250 m plus one metre of coordinate-rounding slack),
which is thus complete for the stated 100 m road-proximity envelope; the final projection is exact
within the stored polyline geometry. The guarantee assumes the producer reports zero dropped snap anchors;
shipping pack jobs treat any quadtree split-floor capacity warning as a failed coverage audit rather
than silently claiming complete lookup coverage.

**Version 14** (issue #1420) makes **a map one file**. Three changes, all of them to how something
is *addressed*; the interior of every §5 chunk, §7 record and §8 record is byte-identical to v13.

1. **Global offsets are scaled.** Every offset that addresses the *file* — the header's section
   offsets, the LOD table's `Index Offset`, each LOD's per-chunk offset table (§5.1), and the POI
   and nav directories' offsets — stays a `uint32` but now counts **`2^scale`-byte units** instead
   of bytes. A new header byte carries `scale` as a base-2 logarithm (§1.1); producers write `4`, so
   a unit is 16 bytes and a file's addressable interior is `2^32 × 16 = 64 GiB`. Arithmetic *inside*
   a chunk or a record is untouched — it never leaves the `Chunk Size`-or-512-byte window it always
   had — and neither is any count, id or length.
2. **Terrain embeds.** The header gains a `Terrain Offset` / `Terrain Length` pair (§1.3): a scaled
   pointer to a region holding one [OBCT](OBCT_Spec.md) container verbatim, or `0` for a map with no
   elevation. `obc-dem` still bakes OBCT and the OBCT interior is unchanged — the assembler splices
   the bytes in, and a reader hands the terrain consumer a window onto them rather than parsing
   them.
3. **An edge is addressed by chunk and ordinal** (§8.4). `Edge Id` was the record's pool-relative
   **byte** offset, the one place scaling would have cost more than it bought — a 19-byte minimum
   record cannot afford a 16-byte grain. It becomes a packed `(chunk_index, ordinal)` pair instead:
   27 bits naming the 512-byte chunk, 5 bits naming the record's position inside it. The pool's
   reach goes from `2^32` bytes to `2^36`, which is exactly the interior, in exchange for a walk of
   at most 25 steps over a buffer the reader has already read. The edge record itself does not move
   a byte.

Together they retire the reason a logical map used to be a **set**: a manifest plus several physical
files, split so that no file's `uint32` offsets overflowed and no FAT32 file limit was crossed. The
flat store ([`FLAT_Store_Format.md`](FLAT_Store_Format.md)) removed the filesystem half of that
ceiling and this version removes the format half. There are no shards, no roles, no sectioning and
no set manifest: **one map is one OBCM object.** Its navigation section may span the whole 64 GiB
interior instead of having to fit whatever one shard could hold, which is what made the map-size
ceiling a statement about the nav graph alone. **No sub-region ceiling sits under that number**:
change 3 is there so that the last `uint32` byte offset in the format did not quietly become the new
limit the moment the old one lifted.

**v14 is the only supported version**; earlier maps get repacked.

**The version byte is the hard cut, and it cuts in both directions.** A reader MUST check `Version`
before it reads any byte behind it and MUST refuse anything other than `0x0E`, whether the value is
older or newer than its own: a v13 file (`0x0D`) is refused by a v14 reader because its offsets mean
bytes, and a v14 file is refused by every v13 reader because its offsets do not — the same
mis-parse, seen from the two sides. The refusal is the file's, not the section's: nothing is
partially readable across the cut, because a section offset that means the wrong unit lands
somewhere plausible rather than somewhere obviously wrong. This is also now the **only** place the
map version is stated, since the set manifest that used to carry a copy of it is gone.

**Within v12** (issue #1095, same elevation epic) two of the style record's reserved
flag bits gained meanings — bit 4 **fixed width** and bit 5 **terrain layer** (§2).
This is deliberately *not* a version bump: nothing about the record's length, layout
or any offset moves, and §2's reader obligation for undefined style bits has always
been to ignore them, so a reader that does not know these two parses the same fields
and draws a slightly different-looking contour. §2 carries the argument in full.

**Version 9** (epic #533 N2) is a §8-only bump that makes the router **bike-type
aware** and shrinks the section it reads (measured ~58% padding in v8 node
chunks). The header stays 40 bytes; everything new hangs off the nav directory,
which grows 22 → **28 bytes** to add a **Profile Table Offset/Count** (§8.6). The
byte-level changes: each way now carries a packed **`way_kind`** class byte
(5-bit highway class + 3-bit surface class) on both its adjacency entries and its
edge record; neighbor entries slim **20 → 15 bytes** by storing each neighbor's
coord as an `int16` delta from the record's own coord and its cost as a `uint16`;
nav chunks are **pinned to 512 bytes** (the reader rejects any other value); node
chunks are **bin-packed** so distinct index leaves may share a chunk; and a
per-map **profile table** (§8.6) of `1..=8` bike profiles is baked in. (v9 was a
hard cut from v8; earlier versions v8 down to v2 were dropped — old maps get
repacked. v10 and then v11 superseded it, see above.)

## Design principles

1. **Pyramid layers.** Each LOD is independent: zoomed out ⇒ read one small
   coarse layer. (vs. tagging every feature with a min-zoom in a single fine
   tree, which forces the MCU to decode fine chunks just to skip them.)
2. **RGB565 in the file, quantized at render.** The style table is
   device-independent and matches the web builder editor. The renderer quantizes the
   small style palette to the target display depth once at load (RGB222 /
   64 colors for the LS021B7DD02).
3. **Meters-per-pixel LOD selection.** Each LOD stores a ground-meters-per-pixel
   threshold; the renderer computes current m/px from zoom + display size and
   picks the level. The same file looks right on a 1024 px desktop and a 240 px
   device.
4. **No runtime discovery.** Every section is reached via an explicit offset and
   every count is stored, so a no_std reader does zero traversal/sizing work to
   parse the structure.

All coordinates are integer **microdegrees** (1e-6 degrees). Projection to
screen space is the renderer's responsibility, not the format's.

## File layout

```
[Header]                            (49 bytes, fixed)
[Style Table]                       (global — shared by all LODs)
[LOD Table]                         (LOD Count entries)
[LOD 0 Index][LOD 0 Offset Table][LOD 0 Data Chunks]    (coarsest)
[LOD 1 Index][LOD 1 Offset Table][LOD 1 Data Chunks]
...
[LOD N-1 Index][LOD N-1 Offset Table][LOD N-1 Data Chunks] (finest)
[POI Directory][POI Indexes + Chunks] (§7)
[Hours-Pool Section]                  (§7.5)
[Nav Directory][Profile Table][Node Index + Chunks][Edge Pool][Snap Index + Chunks]  (§8)
[Terrain Region]                      (§1.3 — an OBCT container, absent when the header says 0)
```

Every structure a header or directory offset reaches begins on a **unit boundary** (§1.1), so the
brackets above are separated by `0..U-1` bytes of `0xFF` filler wherever the previous one did not
end on one. §1.2 states that rule once and what it costs.

The byte layout is produced by `host/obc-pack/src/serialize.rs` (`serialize_lods`) and parsed by
`firmware/obc-reader/src/reader/mod.rs` plus `firmware/obc-reader/src/reader/nav.rs`. All multi-byte
integers are **little-endian**.

---

## 1. Header (49 bytes)

Packed as `struct "<4sBiiiiIBIHIIBII"`.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCM"` |
| 4 | Version | 1 | `uint8` | `0x0E` |
| 5 | Min Lat | 4 | `int32` | Global bbox min latitude (microdegrees) |
| 9 | Min Lon | 4 | `int32` | Global bbox min longitude |
| 13 | Max Lat | 4 | `int32` | Global bbox max latitude |
| 17 | Max Lon | 4 | `int32` | Global bbox max longitude |
| 21 | Style Offset | 4 | `uint32` | **Scaled** offset to the Style Table |
| 25 | LOD Count | 1 | `uint8` | Number of LOD levels (≥ 1) |
| 26 | LOD Table Offset | 4 | `uint32` | **Scaled** offset to the LOD Table |
| 30 | Marker Color | 2 | `uint16` | User-position marker color (RGB565) |
| 32 | POI Section Offset | 4 | `uint32` | **Scaled** offset to the POI Directory (§7) |
| 36 | Nav Graph Offset | 4 | `uint32` | **Scaled** offset to the Nav Directory (§8) |
| 40 | Offset Scale | 1 | `uint8` | **v14**: base-2 logarithm of the offset unit in bytes, `0..=9`; producers write `4` (§1.1) |
| 41 | Terrain Offset | 4 | `uint32` | **v14**: scaled offset to the embedded OBCT region, or `0` for a map with no elevation (§1.3) |
| 45 | Terrain Length | 4 | `uint32` | **v14**: that region's length **in units**; `0` exactly when `Terrain Offset` is `0` |

Note the bbox field order in the file is **lat, lon, lat, lon**. A **scaled** offset is a count of
`2^Offset Scale`-byte units, not of bytes — §1.1 is the whole of that rule, and it applies to every
field this document marks that way, here and in the LOD table (§3), the offset tables (§5.1) and the
POI (§7.1) and nav (§8.1) directories.

The header is 49 bytes, which is not a whole number of units at any scale above `0`, so the Style
Table begins at the first unit boundary at or after it — `64` at the default `U = 16`, giving
`Style Offset = 4` — and the `49..64` gap is `0xFF` filler (§1.2). Reading `Style Offset` rather than
assuming the section follows the header is what it was always for; v14 is simply the first version
where the two differ. The POI and nav sections are always present, so neither of their offsets is
ever `0` — a map with no POIs (or no routable ways) writes an **empty** directory there instead.
`Terrain Offset` is the one offset that may be `0`, and §1.3 says why that is unambiguous.

### 1.1 Offset scale

`Offset Scale` is the base-2 logarithm of the **unit** every scaled offset in the file counts:

```
U           = 1 << Offset Scale        # bytes per unit
byte_offset = u64(field) * U           # 64-bit arithmetic, always
```

A reader MUST widen before it multiplies. `u32(field) * U` is the one way to get this wrong, and it
is wrong silently: the product wraps and lands inside the file rather than outside it, so the read
succeeds and returns the wrong section.

Producers write `4`, so `U = 16` and a file's addressable interior is `2^32 × 16 = 64 GiB`. Legal
values are **`0..=9`**; a reader MUST refuse any other, with an error **distinct from the version
check** — a scale it cannot resolve is an unreadable file, not an old one, and telling a rider the
map is from a future firmware when the byte is simply corrupt is the wrong answer.

The two ends of that range are the two things a unit sits between. At `0` a unit is one byte and the
arithmetic is v13's exactly, which is what makes this an encoding change rather than a new
addressing scheme.

At `9` a unit is 512 bytes, and the reason the range stops there is arithmetic rather than taste:
**`9` is the largest scale at which `512 % U == 0`.** 512 is both the card block and this format's
own fixed chunk size — §7's POI chunks and §8's node, edge and snap chunks are all 512-byte strides
from their region's start — so while `U` divides 512, every one of those chunk starts falls on a
unit boundary that the region start already established, and those runs carry no filler anywhere
inside them. At scale `10` a 1,024-byte unit no longer divides the stride: chunk `1` of every such
run lands mid-unit, and the format would begin paying alignment cost inside runs that are already
aligned to the medium. The secondary cost points the same way — §5's geometry chunks average about
1,600 bytes (§5.1), so a 512-byte unit already spends about a sixth of one on filler and a larger
unit spends more than the chunk. `9` addresses 2 TiB, past any card the store can hold an object
on, so nothing is given up by stopping there.

Recording it as a **logarithm** is what makes "a power of two" a property of the encoding rather
than a rule someone has to check: no byte in this field names a unit that is not one, and no offset
in the file can name a boundary that is not a multiple of it. It is the same trick, for the same
reason, that `FLAT_Store_Format.md` §4 plays with the card's extent size — a grain that has to scale
with the medium, written once as an exponent, so that outgrowing it is a value and not a format.

One rule binds a producer: **the scale MUST cover the file it writes** — `2^32 × U` MUST be at
least the file's total length. ("At least", not "exceed": the largest legal file is exactly
`2^32 × U` bytes, whose last structure starts no later than `(2^32 - 1) × U` and is therefore still
expressible.) A file whose own bytes reach past what its scale can address is malformed, and
the producer that laid it out is the only party positioned to notice; a reader that never resolves
the last section never sees a thing wrong. Everything in this tree writes `4` today, which is also a
byte-determinism pin: two bakes of the same input agree byte-for-byte, and a map past 64 GiB becomes
a different value in this byte rather than a version bump.

### 1.2 Alignment, filler, and what it costs

A scaled offset cannot name a byte that is not a multiple of `U`, so **every structure a scaled
offset reaches begins on a unit boundary**. That is not a rule a writer obeys; it is a property no
encoding of an offset can violate. What a writer obeys is the consequence: wherever the next such
structure would otherwise begin mid-unit, it writes `0xFF` filler up to the boundary.

Three kinds of gap follow, and none of them is content:

- **between sections** — the 49-byte header and the style table, and any two sections a header or
  directory offset names;
- **before a region's chunks** — a region's chunk data begins at the first unit boundary at or after
  the structure preceding it, which is the index (§7.1, §8.1) or the index plus the offset table
  (§3). The `0..U-1` bytes between them are filler;
- **between offset-table-addressed chunks** (§5.1) — chunk `k`'s content ends at its `0xFF`
  sentinel, and chunk `k+1` starts at the next unit boundary.

A reader never sees any of it. A chunk's content ends at its sentinel, a record's at its own length,
and no offset in the file names a filler byte. `0xFF` is the fill because it is already this
format's "nothing here" byte in every chunked section — the style-id sentinel (§5.1), the POI
subtype sentinel (§7.3), the nav degree sentinel (§8.3), the edge `Pt Count` sentinel (§8.4) — so
filler that *did* leak into a decode path meets a stop rather than a plausible record. Reserved
**fields** are still written `0`: a field is content that means nothing yet, and a gap is not
content at all.

A walk through a chunk ends at that stop **or at the chunk's end, whichever comes first**, and both
halves are needed. §8.7's snap chunk is the case that shows why: 512 bytes hold at most
`floor(512 / 12) = 42` twelve-byte anchors, leaving an eight-byte tail too short for a reader to
read a sentinel *out* of — a record starting there would put its `Edge Id` field at bytes `512..516`.
So the byte count bounds that walk and the sentinel bounds the others, and a reader that relies on
only one of the two is wrong in one section each way.

**What it costs, and it is two costs, not one.**

*Per chunk*, only §5's offset-table-addressed geometry chunks pay. §7's POI chunks and §8's node,
edge and snap chunks are fixed 512-byte strides from an already-aligned region start, and `U`
divides 512 at every legal scale (§1.1), so every one of those chunk starts is a unit boundary
already and the runs carry no filler inside them. A geometry chunk's gap is `0..U-1` bytes, and the
gap `(U - len mod U) mod U` averages **`(U-1)/2 = 7.5`** bytes across the sixteen residues:

| chunk length | average gap at `U = 16` | worst gap |
| --: | --: | --: |
| 512 B | 7.5 B — **1.5 %** | 15 B — 2.9 % |
| ~1,600 B (§5.1's measured average chunk) | 7.5 B — **0.47 %** | 15 B — 0.9 % |
| 4,096 B | 7.5 B — 0.18 % | 15 B — 0.4 % |

`Chunk Size` is capped at `4101` by §5.2's vertex bound, so those three rows are the whole
expressible range: **0.18–1.5 % of geometry bytes, ~0.47 % at the measured average**. (A 16 KiB
chunk would pay 0.05 %, which is where that figure comes from; OBCM cannot express one.) Set against
v11, which removed the 53–65 % of a file that was chunk padding, this gives back under one part in a
hundred of that win, and it is the same trade in the same direction: a few bytes per chunk to stop a
fixed stride from dictating the file's reach.

*Per region and per section boundary*, **everything pays** — one gap of `0..U-1` bytes each,
including the sections that pay nothing per chunk. §8.5's worked example is the honest illustration:
its nav section carries `8 + 12 + 12 = 32` bytes of unit-alignment gap in 2,560 bytes, **1.25 %**,
because a 2,560-byte section is almost all boundary. That ratio is an artefact of the example's
size, not a rate: the count of gaps is a property of the file's *structure* — two per LOD (its index
and its `data_start`) plus a couple of dozen fixed ones across the header, the style and LOD tables,
the six POI categories, the hours pool, the nav section's six and the terrain region — so about 50
in a full-ladder map, a few hundred bytes in total, vanishing against any real map. It is not zero, though, and it is not per-byte, which is the shape a
producer's byte-determinism pin has to encode: the gaps are part of the file, and two bakes agree
on them or they do not agree at all.

### 1.3 The terrain region

`Terrain Offset` and `Terrain Length` are a scaled pointer to a region at the file tail holding one
[OBCT](OBCT_Spec.md) container, byte-for-byte the bytes `obc-dem` bakes and the assembler splices.
Terrain sits last precisely so that splicing it moves no other offset.

> **Terrain is part of the map** (owner, 2026-08-18). **Partial updates are not a supported
> operation**: a terrain re-bake re-emits the map, the same as any other content change. There is no
> terrain-only update path, no separable raster object, and no client obligation to reconcile one —
> a rider taking a new raster is taking a new map, and that is the whole of the contract.
>
> This is stated as a design statement rather than discovered as a limitation, because the
> capability it declines — replace the raster alone, leave the map — existed under the volume-set
> roles for its entire life and was exercised **zero times**. Keeping it would mean carrying a
> second object, its identity, its version pairing and its reconciliation for a hypothetical
> operation, which is the exact complexity class this version deletes. Supporting context, not
> justification: a new Copernicus posting is a yearly event at most, and a full re-send lands inside
> the transfer worst case the no-resume rule already accepts — about twenty minutes over USB
> ([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §1).

`Terrain Offset == 0` means **the map carries no elevation**, and `Terrain Length` MUST then be `0`;
a reader MUST refuse a file that sets one without the other. `0` is unambiguous as an absence
because the header occupies byte `0`, so no region can begin there — which is the same argument this
section already makes for the POI and nav offsets, only turned around: those two are always present,
so `0` is a bug there, while terrain is genuinely optional, so `0` is its answer.

**A reader hands the region over; it does not parse it.** A reader forms a **window** — a byte
source whose offset `0` is the region's first byte and whose length is `Terrain Length × U` — and
gives that to the terrain consumer, which reads it exactly as [`OBCT_Spec.md`](OBCT_Spec.md)
describes reading a terrain file. Nothing about OBCT changes and nothing here restates it: the
container carries its own magic, version, header and offset directory, and every offset inside it is
relative to its own first byte. That is what makes a window sufficient and a copy unnecessary.

Two consequences, and both are what "the window is not the payload" means:

- **The window is up to `U - 1` bytes longer than the container.** `Terrain Length` counts units, so
  it is the container's byte length rounded up, and the tail is §1.2 filler. The container's own
  header bounds its content: a reader MUST NOT derive the payload length from the region length, and
  a consumer MUST NOT read past what the container's own structure addresses.
- **A terrain region that will not parse is not a broken map.** Elevation is an enhancement
  (`OBCC_Spec.md` §13): a reader whose OBCT parse fails MUST fall back to no elevation, MUST still
  mount, render and route, and MUST NOT present the map as faulty. That is exactly the clemency a
  missing terrain sidecar already got, unchanged by the move inside the file — a rider whose raster
  is unreadable has the map they would have had without one. A **writer** gets no such clemency and
  MUST verify the region it splices, which is the same asymmetry between *reading an aged card* and
  *publishing a file* that the sidecar rule drew.

### Marker Color

The **user-position marker** (a chevron drawn at the user's GPS fix, pointing
along their course) is a single global map-presentation property, so its color
lives in the header rather than the per-feature Style Table — the marker is not an
OSM feature. It is RGB565 like every style color and is resolved to a device pixel
through the same render-time color policy (quantized to 64 colors on the
LS021B7DD02, true-color in the simulator). The marker's **shape and size are fixed**
in the renderer; only its color is map-configurable (the web builder editor sets it).
The default is `0xF800` (bright red), which reads well over both sea and land.

---

## 2. Style Table

Maps numeric style IDs to rendering properties. **Global**: style IDs are shared
across every LOD. Packed as `Count`, then `Count` records.

1. **Count** (`uint8`): number of styles.
2. **Style Records** (`Count` × 8 bytes, v10 — v5..v9 were 6 bytes):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | ID | 1 | `uint8` | Style ID, referenced by feature headers |
| 1 | Z-Index | 1 | `int8` | Painter's-order layer (lower drawn first) |
| 2 | Color | 2 | `uint16` | RGB565 (the primary color) |
| 4 | Weight | 1 | `uint8` | Stroke width in pixels (lines) |
| 5 | Flags | 1 | `uint8` | Bits 0-1: priority level (1=highest/render first, 4=lowest/render last). **Bit 2 (v10): dashed** line (else solid; ignored for polygons). **Bit 3 (v10): color2 present.** **Bit 4: fixed width.** **Bit 5: terrain layer.** Bits 6-7 reserved, written 0 — a reader MUST **ignore** them, not reject the record (see below) |
| 6 | Color2 | 2 | `uint16` | RGB565 **secondary color** (v10). Written `0x0000` when flag bit 3 is clear; readers MUST ignore it then (`0x0000` is a legit color — black — not a "no color2" sentinel) |

The **secondary color** and **line style** drive the finest-LOD line/polygon
embellishments (road casing, dashed admin borders, railway stripes, polygon ring
outlines — epic #556); the semantics are the renderer's, not the format's. A solid,
single-color style (flags bits 2-3 clear, `Color2 = 0x0000`) is the pre-v10 record
padded to 8 bytes, so a map that uses no line styles renders identically.

**Bit 4 — fixed width.** `Weight` is the stroke's width in **device pixels**, used
verbatim: the renderer's zoom→width ramp does not apply to this style. It marks a
style as *a mark on the map* rather than *a thing with width on the ground*, which is
a general property and not the property of any one feature type. The ramp exists
because a road genuinely is wider than a footpath and both are wider seen from 1 m/px
than from 100 — a mark has no ground width at all, so ramping it is not merely wrong
but backwards: it draws thinnest where the mark carries the most meaning and thickest
where it does the most damage. The width is still clamped to the renderer's `1..=12`
px range; the bit opts out of the ramp, not out of the panel. Ignored for polygons,
whose fills have no stroke width (their §5 outline accent is a fixed hairline already).

> **Why no shipped style but the contours takes it (yet).** Every other line in the
> shipped presets *is* a thing on the ground — roads, tracks, rail, waterways, admin
> borders that follow ridges and rivers — so the ramp is what they want, and a style
> that opted out would freeze at one width across a 100× zoom range. Contours (#1095)
> are the first shipped mark: a 100 m isoline is a statement about the terrain, not an
> object with a footprint. A future grid, hatch or hairline annotation would take the
> same bit; that is why it is spelled as a property of the style and not as
> "contours draw thin".

**Bit 5 — terrain layer.** The style belongs to the **terrain layer**: the group a
device may suppress wholesale as one user-facing choice, rather than by naming feature
types. It is presentation metadata carried on the style record and nothing else — no
reader behaviour in this version depends on it, and a renderer that ignores it draws a
correct map. It is written so the device Settings toggle (#1096) has something to read.

> **Defining bits 4-5 is not a version bump, and this section is why.** Unlike a
> *feature*'s `Flags` (§5.2), where "a reader MUST reject a feature with any [reserved
> bit] set", the reader obligation for a style record's undefined bits has always been
> to **ignore** them — the reference reader masks bits 0-1 and tests bits 2-3, and has
> never looked at the rest. So a v12 reader meeting a v12 record with bit 4 set parses
> every field correctly and renders a contour at the ramped width instead of the
> authored one: a presentation degrade, inside one style record, with no offset,
> length or count affected anywhere in the file. A version is this format's hard cut —
> it makes every existing map unreadable until repacked and every existing reader
> refuse every new map — and it is reserved for changes that would otherwise be
> *misparsed*, not for ones that are merely rendered older. **Bits 6-7 keep exactly
> this contract**: written `0`, ignored by readers, and definable in place the same way.

> **Style IDs are assigned by the packer, not authored.** A style ID is a
> purely internal reference into this table — no reader depends on a specific
> value, only on global uniqueness within the file. The packer ignores any `id`
> in `config.json` and numbers every feature type sequentially (`1`-based, in
> document order) at load time, so collisions are impossible by construction.
> `0xFF` is reserved as the end-of-features sentinel (see §4), so a file holds at
> most 254 distinct styles.

---

## 3. LOD Table

`LOD Count` entries, ordered **coarsest (index 0) → finest (index N-1)**. Each
entry is 18 bytes, packed as `struct "<fIIHI"`.

| Field | Size | Type | Description |
| :-- | :-- | :-- | :-- |
| Max Meters/Pixel | 4 | `float32` | Upper bound of the m/px range this LOD covers. Strictly decreasing down the list; the coarsest level is `+inf` (`f32::INFINITY`). |
| Index Offset | 4 | `uint32` | **Scaled** offset to this LOD's quadtree index (§1.1) |
| Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the index |
| Chunk Size | 2 | `uint16` | **Capacity bound** of one data chunk (bytes) — per-LOD. v11: not a stride; see below |
| Chunk Count | 4 | `uint32` | Number of data chunks in this LOD |

A LOD's region is three parts:

```
[Quadtree Index]   Index Node Count × uint32          at index_start
[Offset Table]     (Chunk Count + 1) × uint32         at table_start, immediately after the index
[Chunk Data]       unit-aligned chunks                at data_start (below)
```

```
index_start = Index Offset * U                                       # U = 1 << Offset Scale
table_start = index_start + Index Node Count * 4
data_start  = align_up(table_start + (Chunk Count + 1) * 4, U)       # = table_start + ... , rounded up
chunk k     = data_start + offsets[k] * U .. data_start + offsets[k+1] * U
```

where `align_up(x, U) = (x + U - 1) & !(U - 1)`. All of it is `u64` arithmetic (§1.1). Only the last
step needs a word: the index and the offset table are read by 4-byte indexing from a start the
directory names, so neither needs a unit boundary of its own, but the **chunks** are addressed by
scaled offsets, so `data_start` must be one. The `0..U-1` bytes it rounds past are §1.2 filler. At
`Offset Scale = 0` this is v13's arithmetic unchanged, which is the point of writing it this way.

**`Chunk Size` is a bound, not a stride** (v11). It is the packer's leaf-split
threshold and the largest length any single chunk may have; a reader MUST reject a
chunk whose offset pair spans more than it can hold (§5.1 states the bound exactly, which since v14
is `Chunk Size` rounded up to the unit). Chunk lengths come from the offset table
(§5), which is what lets chunks be packed tight — v10's fixed stride is why every
chunk had to be padded to `Chunk Size`.

Storing `Index Node Count` and `Chunk Count` explicitly is what removes any
runtime discovery: the reader never has to walk the tree to learn its size. The
offset table's last entry (`offsets[Chunk Count]`) is the LOD's total chunk bytes,
so one `uint32` read at parse bounds every later chunk fetch.

---

## 4. Quadtree Index (per LOD)

A flat array of `Index Node Count` × `uint32`. **Every LOD's quadtree is built
over the same global bbox** (from the header), so node bboxes are computed
identically at every level and the renderer's subdivision math is
LOD-independent. Coarse levels hold few features ⇒ shallow trees.

Each node value:

- **Leaf** — high bit (`0x80000000`) clear:
  - `0x7FFFFFFF` → **empty** leaf (no chunk).
  - otherwise → the **Chunk ID** into this LOD's data chunks.
- **Branch** — high bit set: `0x80000000 | first_child_index`. The four children
  are stored sequentially in the order **NW, NE, SW, SE**.

Children bboxes are derived by splitting the parent bbox at its **floor-division
midpoints** (`mid = (min + max) // 2` for both axes), matching the packer:

```
NW = (min_lon, mid_lat, mid_lon, max_lat)
NE = (mid_lon, mid_lat, max_lon, max_lat)
SW = (min_lon, min_lat, mid_lon, mid_lat)
SE = (mid_lon, min_lat, max_lon, mid_lat)
```

To query a viewport: start at node 0 with the global bbox, recurse into children
whose bbox intersects the view, and collect `(chunk_id, node_bbox)` for every
non-empty leaf reached. The `node_bbox` is required to decode the chunk (see
§5.2, anchors). A `Chunk ID` addresses the LOD's offset table (§5.1), not a fixed
stride.

---

## 5. Data Chunks (per LOD)

### 5.1 Offset table + tight chunks (v11, scaled in v14)

A LOD's chunk data is addressed by its own **offset table**, written between the
quadtree index and the chunks (§3):

- `Chunk Count + 1` `uint32` entries. Each is a **scaled** offset (§1.1) relative to `data_start`,
  the start of the chunk-data region — so entry `e` names byte `data_start + e * U`.
- `offsets[0]` is always `0`. Offsets are non-decreasing. `offsets[Chunk Count]` is
  the region's total chunk **units**, and `offsets[Chunk Count] * U` its bytes.
- Chunk `k` occupies `data_start + offsets[k] * U .. data_start + offsets[k+1] * U`; its **span** is
  the difference in bytes, and its **content** is the shorter run ending at its sentinel.
- The table is written even when `Chunk Count == 0`, where it is the single `0` entry.

Each chunk is its packed features followed by **exactly one** `0xFF` `CHUNK_END`
sentinel byte, then `0..U-1` bytes of `0xFF` filler up to the next unit boundary (§1.2) — the only
thing v14 adds here, and the reason chunks can be addressed at all past 4 GiB. A `0xFF` style-ID
byte is an impossible style, so the sentinel marks end-of-features for a reader walking
the stream, and the offset-derived end is a second, independent bound behind it. A reader
MUST treat a chunk whose feature stream reaches the offset-derived end **without**
meeting the sentinel as malformed (truncated), not as a clean finish. Because the filler is `0xFF`,
a reader that runs off the end of a chunk's real content stops on a sentinel either way; the
sentinel is what ends the walk, and the span is what bounds it.

A reader MUST validate an offset pair before using it, because `Chunk ID` comes from
a quadtree leaf and is arbitrary in a corrupt map: `k < Chunk Count`,
`offsets[k] <= offsets[k+1]`, `offsets[k+1] <= offsets[Chunk Count]`, and
`(offsets[k+1] - offsets[k]) * U <= align_up(Chunk Size, U)`.

That last bound is the v14 restatement of "a chunk may not span more than `Chunk Size`". A chunk's
*content* still may not exceed `Chunk Size`; its *span* is that content rounded up to a unit, so
`align_up(Chunk Size, U)` — 4,096 for the shipped 4,096-byte bound at `U = 16` — is the tight
bound, and the looser `Chunk Size + U - 1` would admit spans no writer can produce.

> **Why.** v10 addressed chunk `k` at `data_start + k * Chunk Size`, which forces
> every chunk to be padded to `Chunk Size`. Because a quadtree node splits as soon as
> its features overflow one chunk, leaves settle between a quarter and half full, so
> the padding is structural rather than a tuning problem — measured 53% of
> `freiburg.obcm`, 65% of `grimsel.obcm`. One `uint32` per chunk buys all of it back
> (freiburg: 1 534 chunks × 4 B = 6 KB of table for 3.8 MB of padding).

### 5.2 Feature Header (7 or 12 bytes, v11)

`Flags` is at byte **1** in both layouts — its `0x08` **WIDE** bit selects the
layout, so a reader knows the header's width before it reads any field behind it.

**Compact** (WIDE clear), 7 bytes, `struct "<BBBHH"` — the common case:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Style ID | 1 | `uint8` | Reference into the Style Table |
| 1 | Flags | 1 | `uint8` | `0x01` 16-bit deltas · `0x02` polygon · `0x04` has holes · `0x08` WIDE (**clear** here) |
| 2 | Pt Count | 1 | `uint8` | Vertex count of the **exterior** ring, `1..=255` |
| 3 | Anchor X | 2 | `uint16` | Exterior start relative to the **leaf node's min longitude** (microdegrees), `0..=65535` |
| 5 | Anchor Y | 2 | `uint16` | Exterior start relative to the leaf node's min latitude |

**Wide** (WIDE set), 12 bytes, `struct "<BBHii"` — the escape:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Style ID | 1 | `uint8` | Reference into the Style Table |
| 1 | Flags | 1 | `uint8` | Same bits, `0x08` WIDE **set** |
| 2 | Pt Count | 2 | `uint16` | Vertex count of the exterior ring |
| 4 | Anchor X | 4 | `int32` | Exterior start, relative to the leaf node's min longitude |
| 8 | Anchor Y | 4 | `int32` | Exterior start, relative to the leaf node's min latitude |

Bits 4-7 of `Flags` are reserved and written `0`; a reader MUST reject a feature with
any of them set. `Pt Count == 0` is malformed in both layouts. Compact anchors are
**unsigned** — zero-extended, never sign-extended.

A writer MUST choose compact when `Pt Count` is in `1..=255` **and** both anchor
components are in `0..=65535`, and wide otherwise. The escape is not hypothetical: a
coarse-LOD leaf can span far more than 65 535 µdeg (~7 km), so an anchor inside it
genuinely needs the wider field. Everything after the header — hole bookkeeping and
the delta streams — is identical in both layouts and unchanged from v10.

The **anchor** is the feature's first absolute coordinate, stored relative to the
containing leaf node's min corner to keep it small:

```
anchor_abs = (node_bbox.min_lon + AnchorX, node_bbox.min_lat + AnchorY)
```

### Geometry encoding (delta)

Rings are delta-encoded to minimize size. Bit depth is chosen **per feature**:
if every `dx`/`dy` fits in `int8` (|d| ≤ 127), `Flags & 0x01 == 0` and deltas are
`int8`; otherwise the flag is set and all deltas are `int16`.

Polygon rings are **implicitly closed** from their last vertex back to their first. A
writer should therefore omit a repeated final copy of the first vertex; readers and
renderers must accept either representation. Line geometry is never implicitly closed.

- **Exterior ring** (`Pt Count` vertices): the first vertex *is* the anchor;
  the remaining `Pt Count - 1` vertices follow as `(dx, dy)` pairs, each relative
  to the previous vertex.
- **Holes** (only if `Flags & 0x04`, after the exterior deltas):
  - **Hole Count** (`uint8`)
  - per hole: **Pt Count** (`uint16`), then `Pt Count` `(dx, dy)` delta pairs.
    Holes store **all** vertices as deltas — the first is relative to the feature
    anchor, the rest chain from the previous vertex.

Lines use only the exterior ring (`Flags & 0x02 == 0`, no holes).

> **Long-segment densification:** the packer inserts intermediate vertices on any
> segment longer than `30000` microdegrees so that no single delta exceeds the
> 16-bit range. Readers need no special handling — these are ordinary vertices.

> **Per-feature vertex cap:** although `Pt Count` is a `uint16`, a single feature
> (exterior plus all holes, densification included) must not exceed **2048
> vertices**. The reference reader decodes a whole feature into one fixed buffer
> (`MAX_FEAT_PTS`). It validates and consumes the complete encoded feature before
> publishing geometry: if the caller's fixed point/ring scratch is too small, the
> whole feature is dropped with an explicit capacity outcome — no truncated line or
> polygon is exposed. The packer guarantees the format bound through `Chunk Size`:
> a feature can't outgrow its chunk, and its packed bytes are at least
> `7 + 2·(V−1) = 2·V + 5` for `V` total vertices (the smallest header v11 writes is
> the 7-byte compact one, and the densest geometry is 8-bit deltas at 2 bytes per
> vertex after the anchor; holes and the wide header only add). So
> `Chunk Size ≤ (2048−1)·2 + 7 = 4101` keeps every feature within the cap — 5 bytes
> tighter than v10's `4106`, which was derived off the 12-byte header. `obc-pack`
> rejects a larger `Chunk Size` at build time rather than emit a feature the
> reference buffer cannot hold. (The bound is deliberately the *loosest* encoding: a
> genuinely 2048-vertex feature needs the wide header, so it packs to 4106 bytes and
> could never fit a `4101`-byte chunk in the first place.)

> **Per-feature ring cap:** although `Hole Count` is a `uint8`, a single feature
> must not exceed **32 rings** (exterior + 31 holes). The reference reader's ring
> scratch (`MAX_FEAT_RINGS`) is fixed at 32 and a feature past it is dropped whole,
> with the same explicit capacity outcome as the vertex cap. Bytes do not imply
> this bound — a heavily simplified polygon can carry dozens of holes on a handful
> of vertices — so `obc-pack` enforces it structurally: a quadtree node holding an
> over-cap polygon splits (clipping spreads the holes across the children), and at
> the 10 µdeg split floor the smallest holes are dropped to fit.

### Polygon-with-holes byte layout

```
[Feature Header (7 B compact | 12 B wide)]
[Exterior deltas]                ((Pt Count - 1) × (int8|int16) pairs)
[Hole Count (uint8)]
  [Hole 1 Pt Count (uint16)]
  [Hole 1 deltas]                (Pt Count × pairs)
  [Hole 2 Pt Count (uint16)]
  [Hole 2 deltas]
  ...
```

---

## 6. LOD selection (renderer)

The renderer computes the current ground **meters-per-pixel** from zoom and
display size. Using a latitude-based definition, 1 microdegree of latitude ≈
`0.11132` m, so with `zoom` in pixels-per-microdegree-of-latitude:

```
mpp = 0.11132 / zoom
```

Among the LODs whose range covers `mpp` (`Max Meters/Pixel[i] >= mpp`), pick the
**finest** (largest index). The coarsest level's `+inf` always qualifies, so the
result is always valid; clamp to `[0, N-1]`.

Worked example (the 3-level default):

| LOD | content | Max m/px |
| :-- | :-- | :-- |
| 0 country | coastline/land, sea, motorway/trunk, major rivers, admin borders | `+inf` |
| 1 region | + primary/secondary roads, lakes, forests | 50 |
| 2 city/street | + residential/service, footways, buildings, parks | 10 |

- `mpp = 70` → only LOD 0 covers it → **LOD 0**
- `mpp = 30` → LOD 0 & 1 cover it; finest = **LOD 1**
- `mpp = 5`  → all cover it; finest = **LOD 2**

Within a selected LOD, query the quadtree for the viewport, decode the visible
chunks, sort features by style `Z-Index` (painter's algorithm), then draw —
polygons via even-odd scanline fill (holes fall out of the even-odd rule for
free), lines as weighted polylines.

**Backdrop convention.** Before drawing geometry, a renderer clears the screen to
the **backdrop color**: the color of the style with the lowest `Z-Index` (the
bottom of the paint order — in the shipped schema, `natural.land` at `z_index
0`). This is derived from the style table, not a fixed style ID, so it survives
the packer's automatic ID assignment. The shipped packer writes the coastline
complement as `natural.sea` geometry on top; schemas with a different lowest
style remain valid.

---

## 7. POI Section (v7)

Point-of-interest features the packer classifies from OSM nodes and closed-way
centroids (see the category table below). Unlike geometry, POIs are **not**
rendered on the map; the device surfaces them as a category → nearest-list
browser. They are indexed for a nearest-N query, not a viewport walk, so each
category gets its own small quadtree over 36-byte point records (v7 widened them
from 32).

The section is reached from `POI Section Offset` (header offset 32) and is
**always present**: a map with no POIs writes a directory of six empty
categories, never a zero offset. Each POI record carries a `HoursRef` u16 into
the trailing **hours-pool section** (§7.5), reached from the directory's
`hours_pool_offset`.

### 7.1 POI Directory

```
uint8   Category Count            (= 6 in v7)
uint16  Chunk Size                (POI chunk capacity in bytes — the packer writes 512)
per category (Category Count entries, 13 bytes each):
  uint8   Category ID
  uint32  Index Offset            (SCALED offset to this category's quadtree index, §1.1)
  uint32  Index Node Count        (number of uint32 nodes; 0 ⇒ category empty)
  uint32  Chunk Count             (number of data chunks in this category)
uint32  Hours Pool Offset         (SCALED offset to the hours-pool section, §7.5)
uint16  Hours Pool Count          (number of 29-byte blobs; 0 ⇒ no hours in this map)
```

`Chunk Size` is shared by every category (all POI chunks are the same fixed
capacity). As with a LOD, a category's data chunks begin at
`align_up(Index Offset * U + Index Node Count * 4, U)` — the exact §3/§4 convention including v14's
one rounding step, so the reader's
`walk_leaves` leaf-walk and chunk-offset math are reused verbatim. Chunk `k` is then that start plus
`k * Chunk Size`, and because 512 is a multiple of `U` at every legal scale, every POI chunk lands
on a unit boundary without a byte of filler between them. An empty
category (`Index Node Count == 0`) still has a directory entry; its `Index Offset`
points at where its (zero-length) index would start and `Chunk Count` is `0`.

The two **v7 hours-pool fields** trail the per-category entries. `Hours Pool
Offset` is the scaled offset of the hours-pool section (§7.5), so the pool begins at
`Hours Pool Offset * U`; `Hours Pool
Count` is the number of 29-byte blobs there and MUST equal the `count` written at
that offset. `Hours Pool Count == 0` means the map has no hours (the pool is a bare
`0` count); a record's `HoursRef == 0xFFFF` likewise means "no hours."

### 7.2 Per-category quadtree

Identical to the geometry quadtree (§4): a flat `uint32` array using the same
node encoding (branch bit / empty-leaf sentinel / chunk id), built over the **same
global bbox from the header**, with the same floor-division-midpoint NW/NE/SW/SE
subdivision. Point features make these trees small and shallow. A reader walks
one exactly as it walks a LOD index, collecting `(chunk_id, node_bbox)` for each
non-empty leaf; the `node_bbox` is **not** needed to decode a POI record (records
store absolute coordinates), only to prune the walk.

### 7.3 POI records — fixed 36 bytes

Records are packed into `Chunk Size`-byte chunks (512 ⇒ `512 / 36 = 14`
records/chunk). Each record is exactly 36 bytes (v7 widened them from 32). A `0xFF`
**Subtype** byte marks the end of records in a chunk (mirrors the geometry chunk's
`0xFF` style-ID sentinel); trailing bytes of a partial final chunk are
`0xFF`-padded.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Lat | 4 | `int32` | Latitude, **absolute** microdegrees |
| 4 | Lon | 4 | `int32` | Longitude, **absolute** microdegrees |
| 8 | Subtype | 1 | `uint8` | Canonical subtype id (§7.4); `0xFF` = end-of-chunk sentinel |
| 9 | Name Len | 1 | `uint8` | Length of the stored name in bytes (`0` = unnamed) |
| 10 | Name | 24 | `char[24]` | Pre-folded printable ASCII; unused tail bytes are `0xFF` |
| 34 | HoursRef | 2 | `uint16` | 0-based index into the hours pool (§7.5); `0xFFFF` = no hours |

Coordinates are **absolute** (no per-node anchor/delta as in geometry §5): at a
fixed 36 bytes the delta win isn't worth the decode asymmetry with geometry
chunks, and fixed-size records keep chunk packing trivial (`Chunk Size / 36`
records per chunk, no per-record length bookkeeping). The **category** is not
stored per record — it is derived on-device from the subtype (each subtype maps to
exactly one category, §7.4) — and is implicit anyway from which category's
quadtree the record came from.

Names are ASCII-folded at pack time to printable ASCII (`0x20..=0x7E`) and
capped at **24 bytes** (v7 widened the field from 20) — a fixed-width,
one-byte-per-character slot, so the packer transliterates umlauts/accents
(e.g. `ä → ae`) rather than store variable-width UTF-8; an unnamed POI
(`Name Len == 0`) shows its subtype's fallback label on-device. The 24-byte
`Name` field is `0xFF`-padded past `Name Len`.

`HoursRef` is a 0-based index into the hours-pool section (§7.5): blob `i` lives at
`hours_pool_offset * U + 2 + i*29`. `0xFFFF` means the POI has no (parseable) hours.
Duplicate weekly schedules collapse to one pooled blob, so many POIs in a region
can share a single `HoursRef`.

### 7.4 Canonical category / subtype table (normative)

This is the **normative home** of the id table; `obc-formats/src/obcm.rs` is its code
authority for subtype ids, categories, and fallback labels. `obc-pack`'s `poi.rs`
adds only the OSM `key=value` classification that produces each subtype, while the
device reads the shared table directly. **Ids are append-only** — an existing
row's category or subtype id must never be renumbered (an old map's records would
then decode as the wrong POI). Subtype `0` is reserved; `0xFF` is the
end-of-chunk sentinel and can never be a subtype id.

| Category ID | Category | Subtype ID | OSM tag (`key=value`) | Fallback label |
| :-- | :-- | :-- | :-- | :-- |
| 1 | Water | 1 | `amenity=drinking_water` | Drinking water |
| 1 | Water | 2 | `natural=spring` | Spring |
| 1 | Water | 3 | `man_made=water_tap` | Water tap |
| 1 | Water | 4 | `amenity=water_point` | Water point |
| 2 | Campsite | 5 | `tourism=camp_site` | Campsite |
| 2 | Campsite | 6 | `tourism=caravan_site` | Caravan site |
| 3 | Accommodation | 7 | `tourism=hotel` | Hotel |
| 3 | Accommodation | 8 | `tourism=hostel` | Hostel |
| 3 | Accommodation | 9 | `tourism=guest_house` | Guest house |
| 3 | Accommodation | 10 | `tourism=motel` | Motel |
| 3 | Accommodation | 11 | `tourism=wilderness_hut` | Wilderness hut |
| 3 | Accommodation | 12 | `tourism=alpine_hut` | Alpine hut |
| 4 | Resupply | 13 | `shop=supermarket` | Supermarket |
| 4 | Resupply | 14 | `shop=convenience` | Convenience |
| 4 | Resupply | 15 | `shop=bakery` | Bakery |
| 4 | Resupply | 16 | `amenity=marketplace` | Marketplace |
| 5 | Pharmacy | 17 | `amenity=pharmacy` | Pharmacy |
| 6 | Bike shop | 18 | `shop=bicycle` | Bike shop |

Subtype ids are dense and 1-based, so a subtype id indexes directly into the
table (`row = subtype - 1`). The category count in the directory (`6`) equals the
number of distinct category ids; every subtype belongs to exactly one category.

### 7.5 Hours-pool section (v7)

A single deduplicated pool of weekly opening-hours schedules, written after the
last POI category's chunks and reached from the directory's `Hours Pool Offset`
(§7.1). A POI
record's `HoursRef` (§7.3) is a 0-based index into it; identical schedules collapse
to one blob, so a region's shops share entries and the pool stays small (only POIs
with parseable hours cost anything).

```
uint16  Count                     (number of blobs; equals Hours Pool Count in the directory)
per blob (Count entries, 29 bytes each):
  uint8   Flags
  per day (7 days, Mon..Sun, 2 slots each):
    uint8  Open Q                 (quarter-hours from midnight, 0..=96)
    uint8  Close Q
```

Blob `i` (a record's `HoursRef == i`) lives at `Hours Pool Offset * U + 2 + i*29`. An
empty pool is just the 2-byte `Count == 0`. Hours are parsed and normalized from
OSM `opening_hours` **at pack time** (the grammar never runs on the device); the
device does a trivial weekday lookup.

**Blob layout (29 bytes).** `Flags` bit 0 = **seasonal** (the source rule carried a
month/date/season selector and a representative in-season week was baked — the UI
ignores this in v1), bit 1 = **truncated** (a rule the encoding can't model — a
`PH`/`SH` non-`off` rule, `sunrise`/`sunset`, or a 3rd+ interval on a day — was
dropped); other bits reserved `0`. The seven days run **Mon (index 0) .. Sun (index
6)**, each with up to two `(Open Q, Close Q)` intervals.

**Time convention.** A time-of-day is quarter-hours from midnight, `0..=96` (`96` =
24:00), so the resolution is 15 minutes. Per interval:

- **Unused slot** — `(0, 0)`.
- **Closed day** — both slots `(0, 0)`.
- **Open all day (24 h)** — slot 0 `(0, 96)`, slot 1 `(0, 0)`.
- **Overnight wrap** — `Close Q <= Open Q` (both nonzero): the interval runs past
  midnight, stored as-is (never split across days). E.g. `22:00-02:00` → `(88, 8)`.
- A day with more than two intervals is truncated to the first two and the blob's
  `Flags` truncated bit is set.

---

## 8. Navigation-Graph Section (v9)

The **routable graph** the on-device router (epic #116, made bike-type-aware by
#533) runs A\* over: junction **nodes** (derived from OSM node ids shared across
routable `highway=*` ways) joined by undirected **edges** (the polyline between
two junctions, junction-free inside). The packer builds the graph in `nav.rs`
(way-kind classification, bike-legality filter, island pruning, junction split,
dedup, edge splits) and this section is its on-wire form.

The section is reached from `Nav Graph Offset` (header offset 36) and is **always
present**: a map with no routable ways writes an empty directory (`Index Node
Count == 0`) — but still carries its profile table (§8.6), never a zero offset.
Layout, in file order:

```
[Nav Directory]     (40 bytes — the graph's resident header, §8.1)
[Filler]            (0..U-1 bytes of 0xFF — the directory is 40 bytes, §1.2)
[Profile Table]     (§8.6 — 1..=8 bike profiles, always present)
[Filler]            (0..511 bytes of 0xFF in populated files — the producer's 512-byte alignment)
[Node Quadtree]     (§4 encoding over the header global bbox)
[Filler]            (0..U-1 bytes of 0xFF — align_up to the first node chunk, §8.1)
[Node Chunks]       (variable-length junction records, bin-packed, §8.3)
[Edge Pool]         (512-byte chunks; a record is named by (chunk, ordinal), §8.4)
[Filler]            (0..511 bytes of 0xFF)
[Snap Index]        (§8.7 — the sparse exact-edge anchor quadtree, v13)
[Filler]            (0..U-1 bytes of 0xFF)
[Snap Chunks]       (fixed 512-byte anchor chunks, §8.7)
```

Design intent: the device is too RAM-tight for any id → offset table (a real
region has millions of graph elements), so A\* **re-fetches spatially** — settling
a node is one quadtree descent to its coord's leaf + one chunk read — and each
record carries its neighbors' coords **inline** so relaxation (`f = g + h`) needs
no second fetch. Edge geometry is touched while resolving the two exact projected endpoints and
when the final route is emitted; the A\* search between those virtual endpoints still never fetches
geometry.
Only the directory and the profile table (≤ `8 × 56 = 448` B) are resident.

### 8.1 Nav Directory (40 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Index Offset | 4 | `uint32` | **Scaled** offset to the node quadtree index (§8.2) |
| 4 | Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the index; `0` ⇒ **empty graph** |
| 8 | Node Chunk Count | 4 | `uint32` | Number of node data chunks (§8.3) |
| 12 | Edge Pool Offset | 4 | `uint32` | **Scaled** offset to the edge pool (§8.4) |
| 16 | Edge Chunk Count | 4 | `uint32` | Number of `Chunk Size`-byte chunks in the edge pool; **at most `2^27`** since v14, the reach of an `Edge Id`'s chunk field (§8.4) |
| 20 | Chunk Size | 2 | `uint16` | Fixed capacity of every nav chunk — **must be `512`** (the reader rejects any other value) |
| 22 | Profile Table Offset | 4 | `uint32` | **Scaled** offset of the §8.6 profile table |
| 26 | Profile Count | 1 | `uint8` | Number of 56-byte profile records; **`1..=8`** (reader rejects `0` or `> 8`) |
| 27 | Reserved | 1 | `uint8` | `0` (keeps the directory even-sized; no other meaning) |
| 28 | Snap Index Offset | 4 | `uint32` | **Scaled** offset to the §8.7 snap-anchor quadtree index |
| 32 | Snap Index Node Count | 4 | `uint32` | Number of `uint32` nodes in the snap index; `0` ⇒ no interior anchors |
| 36 | Snap Chunk Count | 4 | `uint32` | Number of fixed 512-byte snap-anchor chunks following that index |

Node data chunks begin at `align_up(Index Offset * U + Index Node Count * 4, U)` — the §3/§4
convention including v14's one rounding step, so the reader's leaf-walk and chunk-offset math are
reused verbatim.
The packer writes the **profile table just after this 40-byte directory**
(before the node index), so `Index Offset` and `Edge Pool Offset` point past it. The directory is 40
bytes and `Profile Table Offset` is scaled, so at `U = 16` the table starts at the directory's byte
48 with eight bytes of §1.2 filler between them; at `U = 1` it still starts at byte 40.
For a populated graph, current producers insert `0..511` bytes of `0xFF` filler after the
profile table such that the first node chunk lands on a 512-byte file offset. Because every node
chunk is 512 bytes, this
also makes `Edge Pool Offset` 512-byte aligned. A full-chunk read can therefore
be served by one physical card command instead of the two commands required when
the same logical read straddles sectors. This is a **producer guarantee, not a
reader validity requirement**: every boundary is explicitly addressed by the directory, so a file
that skips the alignment is still valid and merely slower.

**The two alignments do not fight, and it takes one sentence to see why.** 512 is a multiple of `U`
at every legal scale (§1.1 caps it at 512), so a producer that lands its node chunks on a 512-byte
boundary has landed them on a unit boundary too. The index start is the field with something to
satisfy: it must itself be a unit multiple, and `align_up(index_start + 4 × N, U)` must be the
512-byte boundary. Both are satisfiable for every node count `N` — the rounding step is exactly the
slack that makes it so, since it lets the index end anywhere in the `U` bytes below the target
rather than exactly on it. §8.5 works one through.

The edge pool is followed by optional `0xFF` filler, the §8.7 snap index, and its chunks. Producers
align the first snap chunk to a 512-byte file offset just like the node chunks. An empty graph still
writes `Chunk Size` and the profile table, and points all zero-length data offsets just past the
profile table, exactly like an empty POI category. A populated graph with no edge longer than 300 m
sets both snap counts to zero and points `Snap Index Offset` just past the edge pool.

**All of §8's filler is `0xFF` since v14**, where v13 wrote zeros for the 512-byte alignment run and
`0xFF` for the padding inside a chunk. One fill byte, one rule (§1.2): a gap is `0xFF` and a reserved
field is `0`. The alignment run is a gap no offset reaches, so it takes the gap's byte.

**`Chunk Size` is pinned to 512 in v9.** Earlier versions let it vary (up to
2048); v9 fixes it so a leaf holds a handful of junction records — one chunk read
serves one A\* settle — and the reader **rejects a directory whose `Chunk Size`
is not 512** (a distinct parse error from the header version check, so an old
file and a mis-sized current file are told apart). The geometry sections' configurable
`chunk_size` (§5) is independent — that knob governs §5 only; nav is pinned.

### 8.2 Node quadtree

Identical to §4 / §7.2: a flat `uint32` array with the same node encoding (branch
bit / empty-leaf sentinel / chunk id), built over the **same global bbox from the
header**, with the same floor-division-midpoint NW/NE/SW/SE subdivision and BFS
flattening. The packer splits a leaf once its packed records (§8.3) exceed one
chunk — by **bytes**, since records are variable-length — with the same 10-µdeg
recursion floor. As with POIs, a node's `node_bbox` is not needed to decode its
records (coordinates are absolute); the walk only uses it to prune.

**Bin-packed chunks (v9).** After building the tree, the packer assigns chunk ids
**first-fit over the leaves in BFS emission order**: each leaf's record block goes
into the first already-open chunk with room, opening a new chunk only when none
fits (v8 gave every leaf its own chunk, wasting the ~58% of a chunk a half-full
leaf left empty). One consequence is load-bearing:

> **Distinct index leaves may reference the same chunk id.** First-fit reaches
> back to earlier chunks, so leaves sharing a chunk can be spatially distant. A
> walk that visits several leaves sharing a chunk decodes that chunk once per
> leaf, so a consumer may see the same junction record **more than once per
> query** — and see records outside the leaf's own bbox. Consumers must therefore
> be **idempotent**. The reference consumers are: A\* settle matches by `Node Id`
> (a repeat is a no-op), and snap tracks the best candidate (a repeat can't
> change the best). A single leaf's records never straddle a chunk boundary.

The index still stores exactly one chunk id per leaf; only the leaf→chunk mapping
changed (many-to-one instead of one-to-one). The reader's leaf-walk and
chunk-decode are unchanged.

### 8.3 Junction records (variable length)

Records are packed back-to-back into 512-byte chunks; unused trailing bytes are
`0xFF`. A record is `13 + 17 × Degree` bytes:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Lat | 4 | `int32` | Latitude, **absolute** microdegrees |
| 4 | Lon | 4 | `int32` | Longitude, **absolute** microdegrees |
| 8 | Node Id | 4 | `uint32` | Dense pack-run node id (the A\* hash key; stable within one file) |
| 12 | Degree | 1 | `uint8` | Neighbor count; **`0xFF` = end-of-chunk sentinel** |
| 13 | Neighbors | 17 × Degree | | `Degree` entries, layout below |

Per neighbor entry (17 bytes, v12):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Neighbor Id | 4 | `uint32` | The adjacent junction's `Node Id` |
| 4 | Neighbor dLat | 2 | `int16` | Its latitude as a **delta from this record's `Lat`** (µdeg) |
| 6 | Neighbor dLon | 2 | `int16` | Its longitude as a delta from this record's `Lon` |
| 8 | Edge Id | 4 | `uint32` | The connecting edge, §8.4 addressing |
| 12 | Cost M | 2 | `uint16` | The edge's raw ground length in meters (the unweighted distance) |
| 14 | Way Kind | 1 | `uint8` | The edge's packed class byte (§8.6) — the input to profile weighting |
| 15 | Ascent M | 2 | `uint16` | **Directional** (v12): the integrated climb, in metres, of riding this edge *from this record's node toward the neighbor*. Saturating; `0` on a map packed without terrain |

The neighbor's absolute coord is reconstructed as `(Lat + dLat, Lon + dLon)`; the
packer guarantees both endpoints of every edge sit within `int16` of each other
(see §8.4) so the delta never overflows. `Cost M` is the **unweighted** ground
distance; the profile-weighted cost A\* actually accumulates is
`Cost M × effective_multiplier(Way Kind) >> 4 + Ascent M × Climb Weight` (§8.6),
computed on device at relaxation — the file stores distance and climb, not weight.

`Ascent M` is an **integral over the edge's polyline, not an endpoint
difference**. A pass road between two 500 m junctions has hundreds of metres of
climb in each direction and no net change at all; an endpoint delta would price
it as flat. The producer samples elevation along the edge's densified polyline
(one sample per vertex plus interpolated points, so no gap exceeds ~50 m of
ground) and folds the `(distance, elevation)` stream through the shared
dead-banded integrator, so the number a route is *costed* by is the number the
rider is later *shown*. A stretch with no elevation coverage contributes nothing:
the integrator re-anchors across the hole rather than booking the climb over it.

Rules:

- **Sentinel.** Because chunks are `0xFF`-padded, the byte where the next record's
  `Degree` would sit reads `0xFF` — the reader stops there (mirrors the POI
  subtype sentinel; the geometry chunks' style-id sentinel likewise). A record
  never straddles a chunk boundary, so a chunk decodes in isolation.
- **Degree cap: 24.** `13 + 24 × 17 = 421 ≤ 512`, so a cap-degree record always
  fits one chunk; real OSM junction degrees never approach it. A pathological
  node keeps its **first 24** adjacency entries (edge-pool order, deterministic)
  and the packer warns; a dropped arc survives one-way via the neighbor's own
  record. `0xFF` can therefore never be a real degree.
- **Undirected, with one exception.** Every edge appears in both endpoints'
  records with the **same** `Edge Id`, `Cost M`, and `Way Kind`. **`Ascent M` is
  the exception and MUST NOT be assumed equal**: the entry `a→b` carries
  `ascent(a→b)` and the entry `b→a` carries `ascent(b→a)`, which is the first
  direction's *descent*. A consumer that verifies "both sides agree" must exclude
  this field. A self-loop (`a == b`, e.g. a lollipop loop) appears **once** in its
  node's record, carrying its forward direction's ascent.
- **Seam determinism.** A producer that cuts one edge into pieces at a cell border
  (`OBCA_Spec.md` §3) integrates each piece over the **same global elevation
  lattice**, so two neighbouring cells' stubs are each the integral of their own
  geometry over one surface and the pieces' ascents sum to the uncut edge's.
- Degree `0` is valid to decode but the packer never emits it (every junction
  comes from at least one edge endpoint).

### 8.4 Edge pool

*(The **record** is byte-identical to v9/v11; v14 changes only what an `Edge Id` means. The v12
climb lives in the adjacency entry, not here: v13 reads the pool during endpoint projection, but
A\* relaxation still must not have to touch it.)*

Deduplicated edge geometry, fetched at route emit (stitching the A\*
came-from chain into the output polyline) and by v13's endpoint projection; also the sum of `Length M` over the
chain is the route's **displayed** distance — the weighted `g` is no longer a
distance). The pool is a run of `Edge Chunk Count` × 512-byte chunks beginning at
`Edge Pool Offset * U`; records are
packed back-to-back, and a record that would cross a chunk boundary is pushed to
the next chunk start (`0xFF` filler fills the gap), so **no record straddles a
chunk** — one chunk-granular read always covers one edge. Since v14 that rule carries a second
weight: it is what makes "the *n*th record of a chunk" a well-defined thing to name.

**Addressing: `Edge Id` is a packed `(chunk, ordinal)` pair** (v14). The `uint32` splits at bit 5:

```
chunk_index = Edge Id >> 5             # 27 bits
ordinal     = Edge Id & 0x1F           #  5 bits
chunk_start = Edge Pool Offset * U + chunk_index * 512
```

`ordinal` is the record's **position within its chunk**, counting from `0` — not a byte offset into
it. Ids stay opaque to consumers (assigned at pack time, meaningless across files) and the pool
still carries **zero resident index bytes**, which is the property that chose this packing over an
edge-id table in the first place and the property v14 had to preserve.

**Resolving one.** A reader reads the single 512-byte chunk at `chunk_start` and walks `ordinal`
records from its first byte, taking each record's length from its own `Pt Count`. Every record the
walk touches — the intermediate ones and the target alike — gets the **same four checks**, so the
walk is written once and applied `ordinal + 1` times:

```
# `p` is a byte position in 0..=512. Every bound below is written ADDITIVELY on `p`.
step(p):
    if p + 19 > 512:            refuse   # no record fits: 19 B is the format's smallest
    n = u16_at(p + 4)                    # Pt Count
    if n == 0xFFFF:             refuse   # end-of-chunk sentinel: no record here
    if n < 2:                   refuse   # impossible count; also what stops 4*(n-1) underflowing
    len = 15 + 4 * (n - 1)
    if p + len > 512:           refuse   # record claims bytes past its chunk
    return len

p = 0
repeat ordinal times:
    p += step(p)                         # every intermediate record is bounds-checked too
step(p)                                  # the target record, same four checks
```

**`512 - p` MUST NOT appear anywhere in that walk, in any width.** Written as `512 - p < 19` the
guard is a bug in every unsigned language this spec is implemented in: once `p` passes `512` — which
a corrupt `Pt Count` does in a single step — the subtraction wraps to a huge value, the guard passes,
and `u16_at(p + 4)` reads outside the chunk. This is the same class of mistake as the `u32 * U`
narrowing §1.1 warns about, and it is spelled out here because this block is the one a reader
transcribes verbatim.

Four refusal rules, and a reader MUST apply all of them, because an `Edge Id` reaches it from an
adjacency entry or a snap record and is arbitrary in a corrupt map: `chunk_index < Edge Chunk
Count`; no record may start where one cannot fit; the walk MUST NOT pass the chunk's last record (an
`ordinal` past it is invalid, never a neighbouring record — and not a record of the *next* chunk,
which is why the bound is `512` and not the pool's end); and no record may claim bytes past its
chunk. A refused id is a malformed map, not an absent edge.

**`Pt Count == 0xFFFF` is the end-of-chunk sentinel**, and it costs a writer nothing: chunks are
already `0xFF`-filled, so the two bytes at `p + 4` of a gap already spell it. `Pt Count` is at least
`2` in every real record, so `0xFFFF` is impossible content — the same shape as the style-id
sentinel (§5.1), the POI subtype sentinel (§7.3) and the nav degree sentinel (§8.3). A gap shorter
than six bytes cannot be read for it, which is what the `512 - p < 19` test covers: no record fits
there either way.

**Why five bits, and what they buy.** An edge record is `15 + 4 × (Pt Count − 1)` bytes with
`Pt Count ≥ 2`, so the smallest record this format can express is **19 bytes** and a 512-byte chunk
holds at most `floor(512 / 19) = 26` of them. Five bits name `0..=31`, which covers 26 with room to
spare, and leave **27** for the chunk index. Six bits of ordinal would have left 26 for the chunk
and capped the pool at 32 GiB, *below* the interior; five is the split where the two ceilings meet:

```
pool ceiling = 2^27 chunks × 512 B/chunk = 2^36 B = 64 GiB
interior     = 2^32 units × 16 B/unit    = 2^36 B = 64 GiB      (§1.1, at the default scale 4)
```

> **The edge pool's `4 GiB − 1` ceiling is gone, not raised.** A byte offset reached `2^32` bytes;
> `(chunk, ordinal)` reaches `2^36`, **16× further**, which is exactly the interior a scale-4 file
> addresses. At the default scale the pool therefore cannot be the binding limit on anything: a pool
> that big *is* the whole map, and the file's own interior stops it first. The navigation section's
> practical limit is now that shared **64 GiB** interior — no sub-region ceiling sits under it — and
> for scale, a DACH-shaped selection's entire nav-plus-POI content is 2.8–3.0 GiB
> (`OBCA_Spec.md` §1.5), so the figure is about twenty times it.
>
> One honest residual: at a scale **above** `4` the interior grows past 64 GiB while the pool does
> not, so a map past 64 GiB would have interior room its edge pool could not use. That is a limit
> worth naming and not one any map approaches; lifting it would be another bit of chunk index traded
> against an ordinal that has six to give.
>
> `Edge Chunk Count` MUST therefore be at most `2^27`, and a reader MUST refuse a directory that
> exceeds it — no `Edge Id` could name the chunks past that point, so the tail would be bytes the
> directory claims and no id reaches. (That is the same posture `FLAT_Store_Format.md` §6 takes
> toward an extent count its index cannot name.)

**A chunk holds at most 31 records** — a producer MUST NOT write a 32nd, so `ordinal` is never more
than `30`. Today's 19-byte minimum record puts the real maximum at 26, so the cap gives up nothing;
it exists so that the encoding stays sound if a future record ever shrinks. **31 and not 32, and the
one-off matters**, because it is what makes `0xFFFFFFFF` impossible *unconditionally* rather than
conditionally: `0xFFFFFFFF` is ordinal `31` of chunk `2^27 − 1`, and both halves of that are
otherwise legal — `Edge Chunk Count` may be `2^27`, so the last chunk exists, and a 32-record cap
would permit ordinal `31` in it the moment a record shrank to 16 bytes (`floor(512 / 16) = 32`),
which is precisely the case the cap is written for. A cap of 31 removes the ordinal half outright,
so the sentinel's soundness rests on one premise instead of two.

**What the walk costs.** At most 25 steps — a `u16` read and an add each — over a 512-byte buffer
the reader has already fetched, against the byte offset's one division. No extra I/O: the chunk that
holds the record is the chunk that holds every record before it, which is the whole reason the
ordinal is *within a chunk* and not within the pool. The `A*` relaxation path is untouched either
way, because it never fetches geometry (§8's design intent); the walk is paid at endpoint projection
and route emit, where a 512-byte read already dominates it.

**`0xFFFFFFFF` remains an impossible id**, which is what keeps §8.7's sentinel working: it names
ordinal `31`, and the 31-record cap above puts every real ordinal at `30` or below — whatever the
chunk index, whatever a future record's size.

Edge record (`15 + 4 × (Pt Count - 1)` bytes):

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Length M | 4 | `uint32` | Ground length in meters (equals the adjacency entries' `Cost M`) |
| 4 | Pt Count | 2 | `uint16` | Polyline vertex count (≥ 2); `0xFFFF` is the **end-of-chunk sentinel** (v14), never a real count |
| 6 | Way Kind | 1 | `uint8` | The edge's packed class byte (§8.6), same value as the adjacency entries' |
| 7 | Anchor Lat | 4 | `int32` | First vertex latitude, **absolute** microdegrees |
| 11 | Anchor Lon | 4 | `int32` | First vertex longitude |
| 15 | Deltas | 4 × (Pt Count − 1) | | Per vertex: `dlat int16, dlon int16`, chained from the previous vertex |

The polyline runs from endpoint `a` to endpoint `b` inclusive (first vertex = `a`'s
coord, last = `b`'s); a consumer walking the edge from `b` reverses it. Deltas are
**lat-first** like every §7/§8 record (the geometry sections §5 are lon-first —
anchors there are viewport-space `x, y`).

`Pt Count ≥ 2` is what makes the 19-byte minimum record — and therefore the 26-record chunk and the
5-bit ordinal (above) — a property of the format rather than an observation about real maps.

Packer guarantees that make the fixed `int16` deltas, the `int16` neighbor deltas
(§8.3), the `uint16` cost, the no-straddle rule and the ordinal's 5-bit field all hold **by
construction**:

- **Densification.** Any segment whose lat **or** lon delta exceeds `30000`
  microdegrees is subdivided with interpolated vertices — the same threshold as
  §5 geometry and the OBCR track encoding. Readers need no special handling.
- **Edge splits.** `nav.rs` splits any edge whose endpoint-to-endpoint lat/lon
  delta exceeds `32000` µdeg (so the §8.3 neighbor delta fits `int16`) or whose
  `Length M` exceeds `60000` m (so `Cost M` fits `uint16`), into pieces joined by
  **synthetic degree-2 junctions** (new dense ids past the real ones). The
  serializer additionally splits any piece whose densified record would exceed one
  chunk (`Pt Count > (512 − 15) / 4 + 1`, i.e. 125 points) or whose endpoint span
  would exceed the `int16` bound after densification. Because the smallest record is 19 bytes, a
  chunk that survives those splits holds `1..=26` records — inside v14's 31-record cap, and so
  inside the ordinal's `0..=30`. Routing-neutral: each piece's
  `Length M` is re-measured over its sub-polyline, so costs still sum to the
  original.

### 8.5 Worked example

A minimal graph — two junctions `A`(lat 100, lon 200) and `B`(lat 900, lon 800)
joined by one 3-vertex edge of 1234 m and way-kind `0x2A` (tertiary/paved: highway
class 10 `| (`surface class 1 `<< 5)`) that climbs 300 m from `A` to `B` and
re-climbs 42 m of dips on the way back — with one profile "`Road`" (climb weight
10), at the default `Offset Scale = 4` (`U = 16`), with the section at a 512-byte-aligned file
offset `S`. Directory fields are **units**, so each is a byte offset divided by 16; `S` is a multiple
of 512 and therefore of 16, and `s = S / 16` is the section's own scaled address:

```
S+0    Nav Directory (40 B):
         index_offset          = s+31     (byte S+496; node chunks begin at S+512)
         index_node_count      = 1
         node_chunk_count      = 1
         edge_pool_offset      = s+64     (byte S+1024 = S+512 + one 512 B node chunk)
         edge_chunk_count      = 1
         chunk_size            = 512
         profile_table_offset  = s+3      (byte S+48)
         profile_count         = 1
         reserved              = 0
         snap_index_offset     = s+127    (byte S+2032; snap chunks begin at S+2048)
         snap_index_node_count = 1
         snap_chunk_count      = 1
S+40   Filler (8 B, 0xFF)                          the directory ends mid-unit
S+48   Profile Table (56 B):
         profile 0: name="Road"      (12 B, 0xFF-padded)
                    highway[32]       (u8 1/16 multipliers)
                    surface[8]
                    climb_weight=10   (1 B)
                    reserved          (3 B, zero)
S+104  Alignment Filler (392 B, 0xFF)              the producer's 512-byte run
S+496  Node Quadtree (4 B):  [0x00000000]          single leaf → node chunk 0
S+500  Filler (12 B, 0xFF)                         align_up(S+500, 16) = S+512
S+512  Node Chunk 0 (512 B):
         rec A: lat=100 lon=200 id=0 degree=1
                nbr { id=1, dLat=+800, dLon=+600, edge_id=0, cost_m=1234,
                      way_kind=0x2A, ascent_m=300 }                          (30 B)
         rec B: lat=900 lon=800 id=1 degree=1
                nbr { id=0, dLat=-800, dLon=-600, edge_id=0, cost_m=1234,
                      way_kind=0x2A, ascent_m=42 }                           (30 B)
         0xFF × 452                                (padding = sentinel)
S+1024 Edge Pool chunk 0 (512 B):
         edge 0 (chunk 0, ordinal 0 ⇒ edge_id = (0 << 5) | 0 = 0):
           length_m=1234  pt_count=3  way_kind=0x2A  anchor=(lat 100, lon 200)
           deltas: (+400,+300) (+400,+300)          → (500,500), (900,800)   (23 B)
         0xFF × 489                    (filler; its pt_count reads 0xFFFF = end of records)
S+1536 Alignment Filler (496 B, 0xFF)
S+2032 Snap Quadtree (4 B): [0x00000000]            single leaf → snap chunk 0
S+2036 Filler (12 B, 0xFF)                          align_up(S+2036, 16) = S+2048
S+2048 Snap Chunk 0 (512 B):
         four 12-byte interior anchors naming edge_id=0   (ceil(1234 / 300) = 5 intervals)
         0xFF × 464                                 (padding = sentinel)
```

The section still ends at `S+2560`; v14 moved bytes inside it and added none. Two of the offsets are
worth checking by hand, because they are the two the scaling actually constrains:

- **`profile_table_offset`.** The table used to sit at `S+40`, immediately behind the directory. `40`
  is not a multiple of 16, so the offset could not name it; the table moves to `S+48` and the eight
  bytes behind the directory become filler. This is the whole cost of scaling in this section.
- **`index_offset`.** The producer wants the first node chunk at `S+512`, and the reader computes it
  as `align_up(index_offset × 16 + 1 × 4, 16)`. Working backwards, `index_offset × 16` must lie in
  `(S+492, S+508]` and be a multiple of 16, which leaves `S+496` — so `index_offset = s+31`, the
  index occupies `S+496..S+500`, and twelve bytes of filler carry it to the boundary. v13 put the
  index at `S+508` with no filler at all; the rounding step is what lets both alignments hold at
  once, for **every** node count, and it costs `0..15` bytes once per region.

Node `A` reconstructs neighbor `B` as `(100 + 800, 200 + 600) = (900, 800)` — no
edge fetch needed for `h`. `edge_id = 0` means the same record it meant in v13, by arithmetic rather
than by coincidence: the only edge is the first record of the first chunk, and `(0 << 5) | 0` is `0`
just as pool byte offset `0` was. A second edge behind it would be `edge_id = 1` under v14 where
v13 called it `23`. Both directions of the edge carry `edge_id = 0`,
`cost_m = 1234` and `way_kind = 0x2A`; only `ascent_m` differs, and that is the
v12 exception above — the same road costs 300 m of climb uphill and 42 m down.
Under "`Road`" the uphill arc weighs `(1234 × 16) >> 4 + 300 × 10 = 4234` and the
downhill one `1234 + 42 × 10 = 1654`. Fetching the edge decodes the polyline
`(100,200) → (500,500) → (900,800)`, its way-kind `0x2A`, and its 1234 m length in
one ≤ 512-byte read.

### 8.6 Profile table (bike-type routing)

`Profile Count` (1..=8) consecutive **56-byte** records at `Profile Table Offset`,
one per selectable bike profile (Road / Gravel / MTB / Touring by default). The
device picks one by index; A\* weights each edge by it. The table is **always
present** — even an empty graph carries ≥ 1 profile — and the reader rejects a
`Profile Count` of `0` or `> 8`.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Name | 12 | `char[12]` | UTF-8, `0xFF`-padded (the §7.3 POI-name convention) |
| 12 | Highway Multipliers | 32 | `uint8[32]` | Weight per **highway class**, `1/16` fixed-point; `16` = 1.0×, `0` = **forbidden** |
| 44 | Surface Multipliers | 8 | `uint8[8]` | Weight per **surface class**, same encoding |
| 52 | Climb Weight | 1 | `uint8` | **v12**: flat metres charged per metre of §8.3 `Ascent M`. `0` = climb-blind |
| 53 | Reserved | 3 | `uint8[3]` | Written `0`; readers MUST ignore |

Stock values are Road `10` / Gravel `8` / MTB `6` / Touring `8` — a road rider
detours further to avoid a climb than a mountain biker does. `0` is a legal and
meaningful value: it reproduces v11's costing exactly, and it is what a producer
writes when it has no opinion.

The **effective multiplier** for an edge whose packed `Way Kind` is `k` is:

```
mh = highway_mult[k & 0x1F]      # low 5 bits = highway class (0..=31)
ms = surface_mult[k >> 5]        # high 3 bits = surface class (0..=7)
effective = (mh × ms) >> 4       # u32 math; 16×16>>4 = 16 = 1.0×
```

The edge is **forbidden** (not routable under this profile) if either byte is `0`.
The weighted A\* cost of the edge is

```
weighted = (Cost M × effective) >> 4  +  Ascent M × Climb Weight     # saturating
```

(saturating into the `uint16` frontier cost exactly as v8 did).

**Admissibility invariant (normative).** Every **non-zero** multiplier is `≥ 16`
(i.e. `≥ 1.0×`). This keeps the great-circle heuristic admissible, so the existing
`ε = 1.3` bound survives — now meaning "≤ 1.3× the best route *under the profile*".
The packer **rejects** a config whose quantized weight is non-zero but `< 16` with
an error naming this A\* heuristic bound; the reader **clamps** a non-zero
multiplier `< 16` up to `16` defensively (a hand-forged file can't hand the router
an inadmissible weight).

**The climb term is additive and non-negative (normative, v12).** `Ascent M` and
`Climb Weight` are both unsigned and the term is *added*, so a descent MUST NOT
reduce an edge's cost below its profile-weighted ground length. That is what keeps
the great-circle heuristic admissible in the presence of elevation — a
descent-credit formulation would let an edge cost less than the straight-line
distance the heuristic assumes, and the `ε`-ladder's guarantee would go with it.
`Climb Weight` therefore needs no lower bound the way a multiplier does: every
`uint8`, `0` included, is admissible. Range check: the worst real edge (60 km,
3000 m of ascent, the §8.4 split bounds) at `Climb Weight = 15` is
`60 000 + 45 000`, inside the existing saturating arithmetic.

#### Canonical way-kind table (normative)

`Way Kind = (surface_class << 5) | highway_class`. This mirrors the packer's single
source of truth (`obc-pack/src/nav.rs` — `highway_class` / `surface_class` /
`classify`); profile configs and the web builder key their multipliers by these
class names.

**Highway class** (5 bits, `0..=31`; `0..=13` assigned, `14..=31` reserved):

| id | class | OSM `highway=` |
|----|-------|----------------|
| 0  | cycleway | `cycleway`, `cycleway_link` |
| 1  | path | `path`, `path_link` |
| 2  | track | `track` |
| 3  | footway | `footway`, `pedestrian`, `footway_link` |
| 4  | steps | `steps` |
| 5  | bridleway | `bridleway`, `bridleway_link` |
| 6  | living_street | `living_street`, `living_street_link` |
| 7  | residential | `residential` |
| 8  | service | `service`, `service_link` |
| 9  | unclassified | `unclassified`, `road` |
| 10 | tertiary | `tertiary`, `tertiary_link` |
| 11 | secondary | `secondary`, `secondary_link` |
| 12 | primary | `primary`, `primary_link` |
| 13 | trunk_cycl | `trunk`/`trunk_link` **only when** `bicycle=yes` |

**Surface class** (3 bits, `0..=7`):

| id | class | OSM `surface=` |
|----|-------|----------------|
| 0  | unknown | absent / unrecognized |
| 1  | paved | `paved`, `asphalt`, `concrete`, `paving_stones`, `concrete:plates`, `concrete:lanes` |
| 2  | compacted | `compacted`, `fine_gravel` |
| 3  | gravel | `gravel`, `pebblestone`, `unpaved` |
| 4  | dirt | `ground`, `dirt`, `earth` |
| 5  | rough | `sand`, `mud` |
| 6  | cobbles | `cobblestone`, `sett`, `unhewn_cobblestone` |
| 7  | grass | `grass`, `grass_paver` |

**Bike legality** (which ways make it into the graph at all): a way is dropped when
`highway=motorway|motorway_link`; `highway=trunk|trunk_link` without `bicycle=yes`;
`motorroad=yes`; `bicycle=no|use_sidepath`; or `access=no|private`. Everything else
— including `footway`/`steps` (legal to *walk* a bike) — is kept; preference (not
legality) is the profile's job.

### 8.7 Sparse exact-edge snap index (v13)

The edge pool is followed by a second quadtree index — at `Snap Index Offset * U`, scaled like every
other directory offset — and `Snap Chunk Count` fixed 512-byte chunks beginning at
`align_up(Snap Index Offset * U + Snap Index Node Count * 4, U)`, the §8.1 convention verbatim.
The quadtree has §8.2's identical flat encoding, global bbox, subdivision, split floor and first-fit
leaf bin packing. Consequently distinct leaves may reference one shared chunk and readers MUST
filter records by their absolute coordinate.

Each record is 12 bytes:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Lat | 4 | `int32` | Anchor latitude, absolute microdegrees |
| 4 | Lon | 4 | `int32` | Anchor longitude, absolute microdegrees |
| 8 | Edge Id | 4 | `uint32` | Pool-relative id of the §8.4 edge geometry to project |

Unused chunk tails are `0xFF`; `Edge Id == 0xFFFFFFFF` is the sentinel, which §8.4 shows stays
impossible as a real id under v14's `(chunk, ordinal)` packing. A final serialized edge
piece contributes no record when its measured geometry is at most 300 m. Otherwise the producer
chooses `ceil(length / 300)` equal-length intervals along the polyline and writes the `intervals − 1`
interior boundaries. Thus endpoint/anchor gaps are no more than 300 m without adding routable graph
nodes or changing A* topology.

The coverage guarantee above requires every generated record to reach the index. A producer MUST
report a split-floor leaf overflow and its dropped-record count; a map release claiming complete
100 m lookup coverage MUST have a dropped count of zero.

The anchor coordinate is never returned as the route endpoint. A reader uses it only to obtain a
small candidate `Edge Id` set, projects the requested coordinate segment-by-segment onto each full
§8.4 polyline, selects the nearest projection (lower `Edge Id` breaks an exact distance tie), and
resolves the winning edge's two endpoint node records. Routing represents an interior projection as
a virtual node with two partial-edge arcs; emission clips the first/last polyline at the same stored
segment/fraction. Exact edge projection is the normal endpoint operation; the 251 m query is only
the candidate-discovery window, while the final result is accepted against its true point-to-road
distance (100 m in the reference router).

---

## Reference implementations

- **Format authority (Rust, no_std):** `firmware/obc-formats/src/obcm.rs`
  (version, fixed record lengths, flags, sentinels, POI ids/categories/labels) and
  `firmware/obc-formats/src/io.rs` (checked little-endian primitives + the neutral
  byte-source/sink seam). It contains no reader, packer, cache, or rendering policy.
- **Writer (Rust, std host):** `host/obc-pack/src/serialize.rs` (`serialize_lods`,
  `serialize_tree`, `serialize_poi_section`, `serialize_nav_section`,
  `flatten_nav_tree` (§8.2 bin-packing), `pack_nav_record`, `pack_edge_record`,
  `pack_profile_table`, `pack_feature`, `pack_chunk`, `pack_style_dict`),
  `host/obc-pack/src/poi.rs` (the OSM-tag classifier for the shared §7.4 ids),
  `host/obc-pack/src/hours.rs` (the `opening_hours` parser + 29-byte blob
  encoder + dedup pool for §7.5), `host/obc-pack/src/nav.rs` (the routable-graph
  builder + the canonical way-kind table behind §8.6), and
  `host/obc-pack/src/config.rs` (the `routing` config + profile quantization).
- **Reader + renderer (Rust, no_std):** `firmware/obc-reader` — `reader.rs`
  (`Reader`, `for_each_feature`, `select_lod_for_mpp`, the POI + nav directories +
  the profile table in `MapTables`, `for_each_nav_node`, `NavNeighbor` delta
  decode, `nav_edge`, `MapProfile::multiplier`, `MapProfile::climb_weight`) — and
  `firmware/obc-render`
  (`Viewport`, `RenderScratch`). Format-contract tests in
  `firmware/obc-reader/tests/format.rs` (byte pins) and
  `host/obc-pack/tests/nav_round_trip.rs` (writer↔reader §8 round trip, incl.
  the profile table, kinds, delta reconstruction, and the bin-packing fill floor).
