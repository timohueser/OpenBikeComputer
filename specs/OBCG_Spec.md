# OBCG v1 published precipitation grid specification

Status: **normative** for format version 1. The Rust byte authority is
`firmware/obc-formats/src/obcg.rs`; the producer is `host/obc-wx-bake`. The Swift implementation
is an independent consumer of this document. A consumer in another language must not need any
implementation.

OBCG is the static object the OpenBikeComputer weather service publishes to object storage. One
OBCG object is exactly **one frame of one shard**: one real UTC validity timestamp, one regular
latitude/longitude window of the one published lattice. There is deliberately no in-object
multi-frame table — the frame set, its keys, its presence and its integrity data live in the
service manifest (§10).

An object carries **no provenance**. The publisher normalises every source it ingests onto one
global lattice at bake time — coarse sources cell-replicated, overlaps resolved per cell by a fixed
priority table that is publisher configuration and never consumer policy — so there is nothing for
an object to name and nothing for a consumer to select between. A cell of a frame is the best
available value there, and which source produced it is information the format removes rather than
transports. Intensity code 15 (§4.1) remains the honest answer for genuinely missing data.

The consumer is an HTTP Range client that must never download a whole frame to answer a corridor
question. The layout is therefore three sections — a fixed self-CRC'd header, a paged tile
directory whose pages verify independently, and tightly packed tile payloads each carrying its
own CRC — such that corridor extraction is: read the header, compute the covering directory
pages arithmetically, read those pages, read the needed tiles. Every read is independently
verifiable; the whole-object CRC additionally serves full-object consumers such as the baker's
own post-encode self-check.

The words MUST, MUST NOT, SHOULD and MAY are normative. All multibyte integers are **little
endian**. All offsets are absolute from byte zero. Integers are unsigned unless their type begins
with `int`. Unix timestamps are signed 64-bit seconds since 1970-01-01T00:00:00Z; leap seconds
are not represented. Coordinates are signed integer microdegrees (10^-6 degrees). Strings and
floats do not occur anywhere in v1.

## 1. Design and limits

- Format offsets and lengths are `uint32`. A reader MUST use checked addition and multiplication
  and MUST reject a value it cannot represent or address.
- Grid geometry is exact integer data: a south/west edge in microdegrees plus per-axis cell
  strides in microdegrees plus cell counts. North and east edges are derived, never stored, so
  the four bounds can never disagree with the cell lattice.
- The tile edge is a **power of two** between 16 and 256 cells, chosen by the producer so
  directories stay small and tiles stay corridor-sized; the manifest states the one value every
  object of a generation carries (§10.2). Tiles reuse the canonical
  WX2 4-bit intensity table and raw4/RLE4 codecs (`OBCW_Spec.md` §6-§7), generalized from 256
  cells to `tile_edge^2` cells; `obc-formats::precip4` is the one shared authority for those two.
  OBCG adds a third codec of its own (§5, deflate over the raw4 nibbles) which **OBCW does not
  have**: it is decoded by the phone, never by the device, and it lives above the shared
  authority precisely so that no LZ decoder can reach the firmware and no OBCW tile can name a
  codec the device cannot decode.
- Dimensions are bounded: `1 <= width, height <= 100,000` and `width x height <= 30,000,000`
  cells (the WX1 decode ceiling). Tile count and every section offset then fit `uint32` with
  margin.
- V1 grids do not cross the antimeridian: `west >= -180 deg` and the derived east edge MUST be
  `<= +180 deg`. That is free under one global lattice, because the shard grid is a plain
  rectangular partition of it and no shard straddles the seam; a *query* that crosses the
  antimeridian is served by the split §10.2a defines, never by an object that wraps.
- An OBCG object carries a **source class** and a reference time (§3.2), and nothing else about
  where its cells came from. Staleness and attribution are manifest data; there is no product id,
  no tier and no per-cell resolution, because there is one dataset and nothing to choose between.

## 2. Canonical file order

A v1 object has exactly this order, with no gaps:

1. 128-byte header;
2. `page_count` fixed-size directory pages;
3. the non-dry tile payloads, tightly packed in ascending tile index order.

`data_offset` MUST equal the checked end of the directory and `total_len` MUST equal
`data_offset + data_len`, which MUST equal the checked end of the final payload; trailing bytes
are invalid.

## 3. Header (128 bytes)

| Offset | Field | Size | Type | V1 rule |
| ---: | --- | ---: | --- | --- |
| 0 | Magic | 4 | `uint8[4]` | ASCII `OBCG` |
| 4 | Version | 2 | `uint16` | `1` |
| 6 | Header Len | 2 | `uint16` | `128` |
| 8 | Total Len | 4 | `uint32` | Exact object length |
| 12 | Reserved | 2 | - | Zero |
| 14 | Flags | 2 | `uint16` | §3.2; exactly one source-class bit |
| 16 | Valid At | 8 | `int64` | Real upstream UTC frame validity time; positive |
| 24 | Reference Time | 8 | `int64` | Upstream run/reference UTC time; positive, `<= valid_at` |
| 32 | South Latitude | 4 | `int32` | South grid edge, `>= -90,000,000` |
| 36 | West Longitude | 4 | `int32` | West grid edge, `>= -180,000,000` |
| 40 | Cell Lat Stride | 4 | `uint32` | Microdegrees per cell northward; nonzero |
| 44 | Cell Lon Stride | 4 | `uint32` | Microdegrees per cell eastward; nonzero |
| 48 | Width | 4 | `uint32` | Cells west-to-east; §1 bounds |
| 52 | Height | 4 | `uint32` | Cells south-to-north; §1 bounds |
| 56 | Cell Size | 2 | `uint16` | The lattice's own cell size in metres; nonzero. See the note below |
| 58 | Tile Edge | 2 | `uint16` | Power of two, `16...256` |
| 60 | Entries Per Page | 2 | `uint16` | `1...1365` |
| 62 | Reserved | 2 | - | Zero |
| 64 | Directory Offset | 4 | `uint32` | `128` |
| 68 | Data Offset | 4 | `uint32` | `128 + page_count x page_bytes` |
| 72 | Data Len | 4 | `uint32` | Sum of all encoded tile payload lengths |
| 76 | Object CRC-32 | 4 | `uint32` | §8 |
| 80 | Header CRC-32 | 4 | `uint32` | §8 |
| 84 | Reserved | 44 | - | All zero |

Derived values, all checked:

```text
north          = south + height x cell_lat_stride        (<= +90,000,000)
east           = west  + width  x cell_lon_stride        (<= +180,000,000)
tile_cols      = ceil(width  / tile_edge)
tile_rows      = ceil(height / tile_edge)
tile_count     = tile_cols x tile_rows
page_bytes     = entries_per_page x 12 + 4
page_count     = ceil(tile_count / entries_per_page)
```

The window is half-open `[south, north) x [west, east)`. Rows advance north and columns advance
east: cell `(col=0, row=0)` has its south-west corner at `(south, west)` — the same orientation
as OBCW. The exact lattice is the microdegree strides; `cell_size_m` is a **metric restatement of
that lattice**, not a claim about any source.

> **`cell_size_m` states the lattice, not a source.** A frame has no single source resolution — its
> German cells come from 1 km radar and its Italian cells from 6.5 km model — so the field states
> the lattice, and the information is *removed* rather than transported:
> there is no per-cell resolution plane, no per-tile source label and no coverage channel. That is
> honest because the mosaic always has a global floor source, so every cell **in the covered
> domain** carries a best-available value; "no radar coverage" renders as model fill, not as dry.
> Intensity code 15 remains the only "we do not know", per §4.1. For the canonical 0.01 degree
> lattice the publisher emits `1113` — 0.01 degrees of latitude, in metres.
>
> The **covered domain** is stated rather than assumed, because the floor's grid is periodic in
> longitude but finite in latitude. The publisher wraps the antimeridian — a global source's column
> east of its last is its first — so every column is covered; it cannot invent the two polar grid
> points the floor does not have, so lattice rows whose centres fall outside ±89.875° have no source
> at all and are published as intensity 15 in every frame. On the canonical lattice that is rows
> 0..11 and 17,987..17,999, 25 of 18,000. A consumer needs no new field for this: those cells are
> already, correctly, "we do not know".

`entries_per_page <= 1365` keeps every directory page (and the header) inside one 16 KiB Range
request: `1365 x 12 + 4 = 16,384` bytes.

### 3.1 The reserved provenance bytes

Bytes 12-13 held a **Product ID** and a **Tier** through the multi-product service: a registry of
source codes, and a radar / model / floor ladder a consumer ranked published products by. Both are
deleted. There is one dataset, one lattice and one cell size, so there is no product to name and no
rank to hold, and a mosaic frame that is 1 km radar over Germany and 27.75 km model over the
Pacific was never a member of a tier in the first place.

The two bytes stay **reserved and MUST be zero**, and a reader MUST reject a nonzero value there
like any other reserved byte (§9 step 1). That is stronger than the rule they used to carry — an
unknown nonzero product id was something a consumer had to tolerate, because the registry was
appendable — and it is deliberate: the field has no meaning left to be forward-compatible about, so
an object claiming provenance there is malformed rather than merely newer. Reclaiming the two bytes
for something else is a format version bump.

### 3.2 Flags

| Bit | Name | Meaning |
| ---: | --- | --- |
| 0 | Observed | Every cell of this object came from an observation, and `valid_at` is the anchor |
| 1 | Forecast | Anything else: model output or a nowcast |
| 2...15 | Reserved | Must be zero in v1 |

Exactly one of Observed and Forecast MUST be set.

**`valid_at` is the frame's place on the cadence, and the flag is what says how the content got
there.** A generation publishes frames at fixed offsets from its reference time (§10.5's `cadence`),
so `valid_at = reference_time + offset_min x 60` and the timestamp is not negotiable: it states
*which instant this frame is about*, not when the data under it was measured. How far the sources
that painted a cell were from that instant is stated once, for the whole generation, as
`freshness`/`cadence.max_source_skew_s` — a consumer that wants to caveat "radar, up to N minutes
old" reads that number instead of assuming one. Re-stamping fetch or bake time as `reference_time`
is forbidden; so is publishing a frame at an offset the cadence does not define.

**A frame ahead of the anchor MUST be painted by forecast source data, and MUST NOT be painted by an
observation carried forward.** A frame at `offset_min > 0` is about an instant no observation of
exists at bake time. What may fill it is anything the publisher classes **Forecast** for its own
source-class bit — model output or a nowcast, which is the same partition the table above draws, no
narrower. A single-frame observation, however recent and however far inside the skew window it sits,
is data about one past instant and is eligible for the anchor alone.

The rule turns on **what the source data is**, not on how near it lands. Being a forecast is the
guarantee; being a forecast of *exactly* this instant is not, and no consumer may read it as one.
`valid_at` states the frame's position on the cadence, while the forecast step underneath it may sit
up to `cadence.max_source_skew_s` away. At a 30-minute window and a 15-minute cadence the quantity
is concrete and worth stating: **one hourly model step paints four consecutive frames.** A step
valid at 11:00 answers 10:30, 10:45, 11:00 and 11:15, because a frame instant at :30 is 1,800 s from
both flanking steps and an implementation that samples the nearest step MUST break that tie toward
the later one — the field valid after the target is about weather that has not happened yet, the one
before it is already past.

What distinguishes this from the frozen-observation case is not the distance: a model step valid at
17:00 is a prediction, and the nearest prediction is a defensible answer for 17:15, whereas a radar
scan of 16:58 is a measurement of 16:58 and is not an answer for 17:15 in any sense. A consumer that
needs the underlying distance reads `max_source_skew_s`; there is no per-frame field for it and none
is coming.

Where no forecast source reaches a forward frame, the honest answer is intensity 15 (§6) — never a
frozen field.

The flag then carries the honesty. A publisher MUST set **Observed** only when both hold:

- every cell of the object came from a source frame that was an observation upstream; **and**
- `offset_min` is `0`.

Under the rule above the second condition is implied by the first: no observation may paint a
forward frame, so no forward frame can satisfy the first condition either. It stays normative and
stays stated, because it is what a consumer validates against and what a publisher's flag decision
must be checkable against without reasoning about that publisher's frame selection.

A consumer MUST NOT infer anything further from the flag than those two facts, and in particular
MUST NOT read Forecast as "unmeasured" or Observed as "exact at `valid_at`". Forecast covers both a
model field and a nowcast of measured origin, and the distinction is not carried.

## 4. Paged tile directory

Tiles are indexed row-major over `tile_rows x tile_cols` with row 0 at the **south** edge:
`tile_index = tile_row x tile_cols + tile_col`. The directory stores one 12-byte entry per tile,
split into `page_count` fixed-size pages of `entries_per_page` entries each; every page ends
with its own CRC-32. The entry-to-tile mapping is pure arithmetic:

```text
page(i)          = floor(i / entries_per_page)
page_offset(p)   = 128 + p x page_bytes
entry_offset(i)  = page_offset(page(i)) + (i mod entries_per_page) x 12
```

so the pages covering any corridor are computable from the header alone.

Entries beyond `tile_count` in the final page are padding and MUST be all-zero bytes; they are
covered by that page's CRC like any other entry bytes.

### 4.1 Directory entry (12 bytes)

| Offset | Field | Size | Type | V1 rule |
| ---: | --- | ---: | --- | --- |
| 0 | Data Offset | 4 | `uint32` | Absolute payload offset, or `0` for a dry tile |
| 4 | Encoded Len | 2 | `uint16` | Payload length; `0` is the all-dry sentinel |
| 6 | Codec | 1 | `uint8` | `0` raw4, `1` RLE4 or `2` deflate4 (§5); `0` for a dry tile |
| 7 | Reserved | 1 | - | Zero |
| 8 | Tile CRC-32 | 4 | `uint32` | CRC-32 of the stored (encoded) payload bytes; `0` for a dry tile |

A codec other than `0`, `1` or `2` is invalid and MUST be rejected; the codec set is closed and
extending it is a format version bump. The tile CRC covers the payload **as stored**, so a consumer
verifies integrity *before* decompressing anything (§9 step 6).

**Dry sentinel.** `encoded_len == 0` declares every cell of the tile to be intensity `0`
(dry). A dry entry has no payload bytes and its other fields MUST be zero — a dry entry is
exactly twelve zero bytes. The sentinel means **dry, never no-data**: a tile of unavailable
cells MUST be encoded (as RLE4 no-data runs), because missing data must never decode as dry
weather. A dry sentinel MUST NOT be used for a partial tile at the north or east grid edge —
such a tile contains no-data padding cells (§5) and MUST be encoded; a reader MUST reject a dry
entry at a partial-edge tile index. Together with §5's all-dry noncanonicality rule this makes
the *dry* representation unique: a tile of dry cells has exactly one legal encoding, the
sentinel, and it can never be confused with a tile of missing data.

**Canonical packing.** Non-dry payloads are stored in ascending tile index order with no gaps:
the first non-dry entry's `data_offset` MUST equal the header `data_offset`, and each subsequent
non-dry entry's `data_offset` MUST equal the previous payload's checked end. The final payload's
checked end MUST equal `data_offset + data_len`. Payloads therefore can never overlap each
other, the directory or the header.

### 4.2 Page CRC

The last 4 bytes of each page are the CRC-32 (§8 parameters) of that page's
`entries_per_page x 12` entry bytes. Any subset of pages verifies independently — a corridor
consumer never needs bytes it did not fetch to prove the integrity of the entries it did.

## 5. Tile payloads

A tile decodes to exactly `tile_edge^2` cells, row-major within the tile, rows advancing north —
the natural sub-grid of §3's cell order. A partial tile at the north or east grid edge still
decodes to the full `tile_edge^2` cells; cells outside the declared width/height MUST be the
no-data intensity `15`, and a consumer MUST clip them.

**Nothing is ever nibble-padded.** `tile_edge` is a power of two in `[16, 256]`, so `N` is always
even and the raw4 image is exactly `N / 2` bytes: no row pads to a byte boundary and no image ends
on a half-byte, at any tile size. An odd grid *width* or *height* changes none of that — it
produces partial edge tiles, and a partial edge tile is still a full `N`-cell square whose
out-of-grid cells carry no-data. The declared width and height clip the decoded square; they never
truncate the encoding.

Three codecs are defined, with the decoded cell count `N = tile_edge^2`. Codecs 0 and 1 are the
canonical WX2 pair of `OBCW_Spec.md` §6.1/§6.2 generalized from 256 cells to `N`; codec 2 is
OBCG's own and does not exist in OBCW.

- **raw4 (codec 0)**: exactly `N / 2` bytes; each byte holds two row-major cells, earlier cell
  in the low nibble. Valid only when the maximal-run RLE4 encoding would be `N / 2` bytes or
  longer.
- **RLE4 (codec 1)**: one byte per run; high nibble `run_length - 1` (1...16 cells), low nibble
  the intensity. Runs MUST be maximal subject to the 16-cell limit (equal adjacent runs only
  after a full 16-cell run), MUST NOT cross the tile boundary, and MUST sum to exactly `N`
  cells — a reader stops as soon as the sum exceeds `N`. The payload MUST be shorter than
  `N / 2` bytes.
- **deflate4 (codec 2)**: a **raw DEFLATE stream (RFC 1951)** whose decompressed output is
  exactly the `N / 2` raw4 bytes of codec 0 — the same nibble packing, without codec 0's
  canonicality restriction. No zlib (RFC 1950) or gzip (RFC 1952) wrapper, no preset dictionary,
  no concatenated streams: the payload is one complete stream, and its last byte is the last byte
  of the payload. The wrapper is omitted because §4.1's tile CRC-32 already covers the stored
  bytes, so a zlib header plus Adler-32 would add six bytes of duplicated integrity to every
  tile; raw DEFLATE is also what both reference implementations speak natively (miniz_oxide's
  `compress_to_vec` / `inflate::core` on the Rust side, Apple's `COMPRESSION_ZLIB` — which is RFC
  1951 despite the name — on the Swift side). Two RFC 1951 details are called out because a
  decoder can get them wrong quietly:

  - **The tile's history starts empty.** A match distance MUST NOT reach before the first byte of
    that tile's own raw4 image — every tile is compressed and decompressed independently, with no
    preset dictionary and nothing carried over from the previous tile. A stream that reaches back
    further is invalid and MUST be rejected; a decoder MUST NOT substitute zeros, the previous
    tile's bytes, or any other fill for the out-of-range distance. (Both reference decoders
    already fail such a stream, and §11 pins one.)
  - **The padding bits of the final byte are not data.** A DEFLATE stream ends on a bit boundary,
    so the bits after the final block's last bit exist only to pad the payload's last byte. They
    are **unconstrained**, and a decoder **MUST NOT** reject a stream on their value. A producer
    SHOULD emit them as zero (the reference producer does), but that is not something to make a
    reader check: an implementation built on a general inflate primitive cannot see those bits at
    all — neither miniz_oxide's `inflate::core` nor Apple's `compression_stream` reports the
    ending *bit* position — while one carrying its own bit-level inflater can. A "MUST be zero"
    rule would therefore not make objects stricter, it would fork readers into two populations
    that disagree about the same published frame, which is the one outcome a two-implementation
    format cannot afford. Six of the eight bit patterns of a real vector's last byte are the same
    object; §11 pins one of them as a positive vector, so a reader that rejects it is provably
    wrong rather than arguably strict.

**Why an LZ codec is here at all.** RLE4 caps runs at 16 cells and has no back-reference, so a
uniform field costs one byte per 16 cells however uniform it is, and 25 identical rows cost 25x
one row. That is exactly the shape of a coarse source upsampled onto a fine lattice, which is
what the baker publishes. DEFLATE collapses both axes: WXR1 measured 5.1x-19.2x over the
raw4/RLE4 pair across a whole global cycle, and up to 42x on upsampled coarse data.

**Codec choice, and what stays unique.** A producer MUST encode each non-dry tile with the codec
that yields the **strictly smallest** payload, breaking a tie in favour of the **lowest codec id**
(raw4 < RLE4 < deflate4). Two of the three resulting rules are checkable by a consumer and are
therefore binding on the reader as well:

- codec 0 is valid only when the maximal-run RLE4 length is `>= N / 2` (as above);
- codec 1 is valid only when its payload is shorter than `N / 2` bytes (as above);
- codec 2 is valid only when its payload is **strictly shorter than the canonical raw4/RLE4
  length of the same decoded cells** — that is, than `min(N / 2, maximal-run RLE4 length)`. The
  consumer computes that length from the cells it has just decoded and MUST reject a codec-2 tile
  that does not beat it. This rule is what keeps RLE4 in use where it genuinely wins — small or
  sparse tiles, where DEFLATE's block overhead loses — rather than leaving it dead weight.

The remaining half of the producer rule, "use deflate4 whenever it *is* strictly smaller", is not
consumer-checkable and is not checked: DEFLATE output depends on the encoder, so a consumer that
demanded a particular compressed length would be pinning an implementation rather than a format.
**A frame therefore no longer has exactly one legal byte image.** V1 guarantees that the *decoded*
frame is canonical — every tile decodes to one cell array, a dry tile has exactly one
representation, codecs 0 and 1 have exactly one payload each for given cells — and that any codec
choice a consumer can disprove is rejected. It does not guarantee that two conforming producers
emit identical bytes. The §11 negative vectors pin the rules that remain checkable.

A full (non-edge) tile whose `N` cells are all dry MUST use the §4.1 sentinel instead of a
payload, under every codec; consequently a decoded payload with every cell dry is noncanonical
and MUST be rejected. (An edge tile is never all-dry — its padding is no-data — and per §4.1 it
is never a sentinel either, so both rules stay disjoint.)

**Decompression is bounded before it is attempted.** A tile decodes to `N` cells and therefore to
exactly `N / 2` raw4 bytes — a number the consumer already has from the *header*. A codec-2
consumer MUST size its output buffer from that number and never from anything the payload claims;
MUST reject a payload of `N / 2` bytes or more before inflating it; and MUST reject a stream that
would write more than `N / 2` bytes, writes fewer, fails to terminate, or leaves input bytes
unconsumed. There is consequently no allocation a decompression bomb can grow, and every one of
those failures is the same verdict: the tile is invalid.

Cell intensities are the canonical 4-bit precipitation table of `OBCW_Spec.md` §7 — the same
codes, the same mm/h thresholds, the same reserved values 13/14 (reject) and no-data 15 (never
dry, never an alert-clear signal). OBCG adds no second quantization authority, and codec 2's
decompressed nibbles are checked against that table exactly like codec 0's.

## 6. Coordinate lookup

For an in-bounds coordinate, cell lookup is exact integer arithmetic on microdegrees:

```text
col = floor((lon_udeg - west)  / cell_lon_stride)
row = floor((lat_udeg - south) / cell_lat_stride)
tile_col = floor(col / tile_edge);  local_col = col mod tile_edge
tile_row = floor(row / tile_edge);  local_row = row mod tile_edge
cell     = decoded_tile[local_row x tile_edge + local_col]
```

Intermediates MUST be checked signed 64-bit (or wider). The north/east edges are outside the
half-open window.

Interpolation follows `OBCW_Spec.md` §5 exactly, so a cell means the same thing on both sides of
the bakery: **data queries MUST sample nearest-neighbour**, using the selected cell exactly, with
no interpolation and no fabricated sub-cell precision; **display MAY interpolate** for legibility;
and **no claim, alert, alert-clear or dry decision may derive from an interpolated value**. Every
corridor extraction in §7 is a data query and is bound by the first rule.

Rationale (2026-08-10): the epic's original rule was no smoothing anywhere, which made 1 km cells
render as hard squares. The rule that was actually load-bearing — that a device never
reports rain, or reports none, on the strength of an invented number — is preserved verbatim for
queries; only the pixels were released.

## 7. Corridor extraction

A corridor consumer performs, in order:

1. **Header read**: the first 128 bytes (any first read `<= 16 KiB` is conforming; 128 bytes
   suffice). Validate §3 including the header CRC before trusting any derived arithmetic.
2. **Directory page reads**: compute the tile index range covering the corridor (§6), map the
   needed indexes to pages (§4), and fetch exactly those pages. Validate each page's CRC and the
   §4.1 entry rules for the entries it uses.
3. **Tile reads**: fetch `[data_offset, data_offset + encoded_len)` for each needed non-dry
   entry, validate the tile CRC over those stored bytes, then decode under the §5 rules — for
   codec 2 that means inflating into a `tile_edge^2 / 2`-byte buffer sized from the header.
   Dry-sentinel tiles cost no read.

Nothing else is required, and a conforming consumer MUST NOT need any byte outside those ranges;
the request-accounting tests in `host/obc-vectors` pin exactly this set. Consecutive needed
ranges MAY be coalesced into fewer HTTP requests; coalescing only ever fetches bytes between two
needed ranges.

A full-object consumer (the baker's self-check, a mirror, a cache validator) instead reads the
whole object and additionally verifies §8's object CRC and the complete §4.1 packing rules over
every entry.

## 8. Integrity

Both CRCs are CRC-32/IEEE (reflected polynomial `0xEDB88320`, initialization and xor-out
`0xFFFFFFFF`; check value `CRC32("123456789") = 0xCBF43926`), the same parameters as every other
OBC format.

- **Object CRC** (bytes 76...79): computed over exactly `total_len` bytes while bytes 76...83
  (both CRC fields) are treated as zero.
- **Header CRC** (bytes 80...83): computed over the 128 header bytes while bytes 80...83 are
  treated as zero. The stored object CRC participates as written, so a header-only reader also
  proves the object-CRC field's integrity.

Writers therefore finish an object by writing both fields as zero, storing the object CRC, then
storing the header CRC. CRC success never excuses structural validation: a malicious producer
can compute valid CRCs over malformed data, so every §3-§5 rule is checked regardless.

Tile and page CRCs (§4) protect partial reads; they are not alternatives to the object CRC but
subsets of the same fail-closed posture.

## 9. Required validation order

A decoder MUST never panic, read outside the announced object or write beyond a tile buffer.
Equivalent early-exit ordering is allowed. A corridor consumer applies each step to the bytes it
fetched; a full-object consumer applies all of them:

1. Validate magic, version, header length, reserved bytes, and the header CRC.
2. Validate flags, timestamps, geometry bounds and §1 limits, tile edge and paging parameters, and
   the derived section layout (`directory_offset`, `data_offset`, `total_len`) with checked
   arithmetic only.
3. For a full object: require `total_len` to equal the available length and verify the object
   CRC.
4. For each directory page used: verify the page CRC; require padding entries beyond
   `tile_count` to be all-zero.
5. For each entry used: validate the §4.1 field rules — a dry sentinel is all-zero; a non-dry
   entry's offset/length lie inside the data section and respect canonical packing.
6. For each tile payload used: verify the tile CRC over the stored bytes **before** decoding or
   decompressing them, then the §5 rules the payload alone decides — a known codec id, reserved
   intensities, maximal runs, exact cell count, the checkable half of the canonical codec choice,
   and for codec 2 the bounded decompressed size, full input consumption and in-range match
   distances.
7. For a full object only: the §5 rules that are properties of the *frame* rather than of one
   payload — the all-dry noncanonicality rule and no-data edge padding, each of which needs the
   tile's grid position to judge.

Step 7 is deliberately not a corridor obligation, and that costs a corridor consumer nothing: an
all-dry payload decodes to the same dry cells the sentinel would have produced, and an edge tile's
out-of-grid cells are clipped by §5 before they can be read. Both are producer mistakes a
full-object validator — the baker's self-check, a mirror, `obc-vectors` — must catch, not
corrections a Range reader has to make mid-ride.

A decoder MUST NOT allocate or reserve memory in proportion to any length a payload claims. The
only sizes it may act on are the header's, and for a codec-2 tile that is `tile_edge^2 / 2` bytes.

## 10. Service manifest

The manifest is the mutable JSON document `wx/v2/manifest.json` beside the immutable frame objects.
Its JSON Schema is checked in at `host/obc-wx-bake/schema/manifest-v2.schema.json` and pinned by
tests; a shared parse fixture both client implementations read is `specs/vectors/wx-manifest-v2.json`.
It is delivery metadata, not a byte format, but its contract is normative.

The service publishes **one dataset**: a global lattice at a fixed cell pitch, cut into a fixed grid
of shards, at a fixed cadence. There is nothing to select between, so the manifest carries nothing
selectable — no products, no tiers, no per-product bboxes, no competing staleness. What it carries
is what a client cannot compute: which generation is current, the constants of the grid, what exists
per frame, and when this stops being usable.

### 10.1 Object keys are computed, not listed

Frame objects are published under immutable keys

```text
<key_prefix>/<generation>/f<offset-min>/s<col>-<row>.obcg
```

where `<generation>` is the cycle's reference time as `YYYYMMDD'T'HHMM'Z'`, `<offset-min>` is
`(valid_at - reference_time) / 60`, and `<col>`/`<row>` address the shard grid from the lattice's
**south-west** corner. The manifest states `key_prefix` and `generation`; the client composes the
rest. This is the one part of the contract a client computes rather than reads, so it is normative
here: an implementation that derives a different string for the same shard is wrong, even if the
object happens to exist.

Shards are addressed by `(col, row)` and not by a flat index. A client derives `(col, row)` from its
bbox by division; a flat index would have it multiply by `shard_cols` to name an object and divide
back to read the presence bitmap, which is two spellings of one identity.

Objects upload first; the manifest swaps last, atomically. A failed cycle leaves the previous
manifest and its objects fully consistent. The publisher's pre-swap proof is **existence and exact
length** at the destination, not a re-read of the bytes: a store that returned wrong content at the
right length would ship a manifest whose `object_crc32` the object does not have. That is a
monitoring gap and deliberately not a consumer one — the client verifies the CRC on every read and
calls a mismatch an error, which §10.3 requires it never to soften into "dry". Frame objects carry long immutable `Cache-Control`; the
manifest's own maximum age is stated in the document (`freshness.manifest_max_age_s`) as well as in
its header, so the rule survives a proxy that rewrites headers.

### 10.2 The grid, stated

`lattice` carries the lattice origin and cell pitch in microdegrees, its width and height in cells,
the shard extent and the shard grid dimensions, the tile edge and paging every object uses, the
`cell_size_m` every object declares, and `covered_rows` as `{start, end}`.

A client MUST take the grid from this block and MUST NOT hardcode it: re-cutting the dataset is then
a baker deploy rather than a client release. `shard_cols` MUST equal `ceil(width / shard_width)` and
`shard_rows` MUST equal `ceil(height / shard_height)`; a client MUST reject a document where they do
not, because it and the publisher would then disagree about which object holds a cell.

### 10.2a Coordinates and the antimeridian

All coordinates in this contract are signed integer microdegrees with latitude in
`[-90000000, 90000000]` and longitude in `[-180000000, 180000000]`. **There is no other
convention**: a 0..360 longitude (`352150000` for `-7.85` degrees) is not a longitude this contract
recognises, and a consumer MUST reject it rather than clamp it or reinterpret it — clamping answers a
corridor from the wrong hemisphere with no error anywhere, which is worse than answering none.

A query window with `west > east` is **not** malformed: it means the window **crosses the
antimeridian**, and a consumer MUST serve it by splitting into `[west, +180)` and `[-180, east)` and
taking the union of the shards each half reaches. `west == east`, `south >= north`, or any
coordinate outside the ranges above are errors.

The lattice itself does not wrap: column `width - 1` and column `0` are neighbours on the globe but
are separate shards, and the shard grid is a plain rectangular partition. Wrapping is a property of
the *query*, resolved by the split above, never of the addressing.

A shard set derived from a bbox is ordered **ascending by `(row, col)`** — including across a
wrap, where the eastern hemisphere's `col 0` therefore precedes the western hemisphere's last
column. Deterministic order is what makes two implementations comparable.

A bbox that is well-formed but does not intersect the lattice yields **no shards**, and a consumer
MUST report that as out-of-domain rather than as an empty result indistinguishable from "everywhere
dry". It MUST NOT clamp the interval onto the lattice edge before testing for intersection: an
off-lattice window clamped first collapses onto the nearest edge shard, and the rider is served
another region's weather instead of being told they are off the map.

`covered_rows` is the half-open range of lattice rows that at least one source reaches. Rows outside
it have no source at all and are published as intensity 15 in **every** frame, permanently — a
property of the dataset, not an outage (§3's covered-domain amendment). Stating it once is what lets
a consumer tell a permanent hole from a broken cycle without a per-cell channel.

The objects there **exist and are listed**; the range is not a second presence channel. What it buys
a consumer is the ability to answer *before* fetching: a bbox whose every row falls outside
`covered_rows` can only decode to "we do not know" in every frame, so a consumer SHOULD say so
directly rather than spend a Range read per frame to learn a permanent fact this field already
stated. A bbox that is only partly outside is ordinary: fetch it, and the uncovered cells arrive as
intensity 15, which is the truth.

### 10.3 What exists: presence, and why a 404 must not mean dry

Each `frames[]` entry carries `offset_min`, the frame's true upstream `valid_at`, a `present`
bitmap, and one `shards[]` entry per present shard with its `bytes`, `object_crc32` and `observed`
flag.

`present` is `ceil(shard_count / 8)` bytes as lowercase hex, first byte first, least-significant bit
first inside each byte. The bit of shard `(col, row)` is at index `row * shard_cols + col`. Bits past
`shard_count` MUST be zero. `shards[]` MUST name exactly the shards `present` names, ascending by
`(row, col)`; a consumer MUST reject a frame where the two disagree rather than reconcile them —
either reconciliation invents a fact about whether an object exists.

The bitmap makes three states distinguishable that a bare `GET` collapses into two:

| bit | shard on the grid | meaning |
| --- | --- | --- |
| set | yes | the object exists. A 404, a short body or a CRC mismatch is an **error** — retry, then surface it. It is never dry. |
| clear | yes | every cell of that shard is dry. There is no object, no request and no failure. |
| - | no | out of domain: the bbox reaches off the lattice. |

A shard that is entirely **no-data** MUST be published, as an object full of intensity 15. Only a
shard whose every cell is dry may be omitted. That is what keeps a clear bit from ever being an
outage in disguise, and it is the whole of "missing is not dry".

`observed` is per shard, not per frame: a mosaic frame is radar over one country and model over the
next ocean at the same instant, and it mirrors the object's own `FLAG_OBSERVED` (§3.2), which the
publisher measures rather than assumes.

### 10.4 Retention: current plus two

`generation` names the current generation and `previous_generations` names the superseded ones whose
objects are still fetchable, **newest first**, at most two. Together they are the complete set of
generations that exist: a lifecycle sweep MAY delete any generation prefix under `key_prefix` that
this document does not name, and MUST NOT delete one it does. Two is what covers a client that
fetched the manifest just before a swap and is still reading the generation it named.

A client MAY finish a read from a listed previous generation; it MUST NOT start planning from one,
because only the current generation's presence and integrity data is in the document.

`previous_generations` MUST hold at most two entries. The cap is normative rather than advisory: a
consumer that saw more would disagree with the publisher's sweep about which generations exist, and
raising it is a manifest version bump, not a configuration change.

**The publisher's obligation, and it is the important paragraph in this section.** An empty
`previous_generations` is a positive claim that no superseded generation exists — it is what makes a
sweep delete. A sweep cannot check that claim: it sees a document, not how the document came to say
what it says. So the guarantee is the **publisher's**, and it is stated as one:

> A publisher MUST write `previous_generations` as the chain carried forward from the manifest
> previously at this key: that manifest's `generation`, followed by its own `previous_generations`,
> truncated to two. It MUST write an empty chain **only** when the key genuinely held no document.
> If a document is present but cannot be read — a torn body, a truncated read, unparseable JSON, or
> a chain entry that is not a generation identifier — the publisher MUST fail the cycle and write
> nothing, leaving the previous manifest and its objects in place.

A torn read must never become a deletion set. Failing the cycle costs one cycle of freshness and the
next tick recovers; publishing an empty chain from a torn read deletes objects in-flight clients are
still Range-reading, and a 404 on a set presence bit is an error by §10.3 — an outage, not a
degradation. A sweep MAY therefore treat any published manifest as authoritative about what exists,
because a publisher that could not honour the paragraph above did not publish one.

### 10.5 Freshness: deadlines, not client constants

`freshness` carries absolute timestamps, so a client compares times and holds no durations of its
own:

- `manifest_max_age_s` — how long a fetched copy of this document may be reused.
- `next_generation_expected_at` — when the next generation is due. Past it, the service is late.
  This is a monitor's alarm; the data is not yet unusable.
- `stale_after` — when this generation stops being usable at all: the validity of its **last**
  frame, past which every frame describes the past. A client past this deadline has *no weather*,
  which is a different thing from no rain: expiry MUST NOT render as dry.

Every timestamp in this document is UTC, and a consumer compares them against **its own clock**.
The service cannot know whether that clock is right, so a consumer whose clock is untrusted SHOULD
treat `generated_at` as a lower bound on the current time rather than declare a fresh generation
expired; a consumer that is confident in its clock SHOULD apply a small tolerance before acting on a
deadline, for the same reason the operational probe allows a few minutes of skew on `generated_at`.
Neither direction may turn into a dry claim.

`cadence` states `frame_step_min`, the number of `frames`, and `max_source_skew_s` — how far from a
frame's stated validity the source that painted a cell may have been. It is a property of the data,
stated so a consumer that wants to caveat "radar, up to N minutes old" reads the number instead of
assuming one.

`attribution` lists every source that may have painted a cell of this generation, in the publisher's
priority order. There is no per-cell provenance (§3's amendment), so every line must be displayable
together; a client MUST NOT treat a `source_id` as selectable.

## 11. Golden and negative material

Checked-in files live in `specs/vectors/` (`grid-*.obcg`) and are described in its
`manifest.json` and README. Positive vectors cover the all-dry sentinel object; one tile per
codec — an incompressible raw4 tile, an RLE4 tile that wins on a tie, an RLE4 tile that wins
outright over deflate4, and a deflate4 tile of upsampled coarse data at tile edge 64; an explicit
all-no-data tile; edge-tile no-data padding; and a multi-page directory with last-page padding.
Negative vectors isolate truncation, bad payload offsets, overlapping/non-canonically packed
payloads, impossible dimensions, a non-power-of-two tile edge, bad paging parameters, an unknown
codec id, overlong RLE, noncanonical RLE, a compressible raw4 tile, a truncated deflate stream, a
deflate stream that over-inflates past the tile's raw4 size, one that under-inflates, one whose
match distance reaches before the start of the tile's raw4 image, a deflate payload that fails to
beat the canonical raw4/RLE4 length, a noncanonical encoded all-dry tile, a nonzero dry sentinel,
a dry sentinel on a partial edge tile, header CRC, object CRC, page CRC and tile CRC mismatches,
and nonzero reserved bytes.

Two positives exist to pin what a decoder must **not** reject: a second legal byte image of the
deflate4 tile, differing only in the padding bits of its final byte, and a `tile_edge = 256` frame
— the production geometry, the only one where a tile payload can exceed 255 bytes and where the
pre-inflate ceiling reaches 32,767.

`specs/vectors/wx-manifest-v2.json` is the shared **manifest** fixture: one v2 document (§10) that
every client implementation parses, so the parsers cannot drift on the one file a rider reads first.
Both must derive the same shard key set from the same bbox against it — that equivalence is the
cross-client test, and it is what "selection is arithmetic" means in practice.

Every vector is a window of the one published lattice — 10,000 x 10,000 udeg cells,
`cell_size_m = 1113` — and bytes 12-13 are zero in all of them; one negative pins that a nonzero
value there is a malformed object rather than an unrecognised code.

Rust builds and validates them through the `obc-formats` authority; Swift independently decodes
the same positives to the same cells and rejects every negative. Vector provenance is recorded
beside the fixtures. Byte or layout changes to this specification MUST land with updated
vectors and both implementations in the same change, per the epic's working agreements.

## 12. Worked size budget

The published lattice is 36,000 x 18,000 cells of 0.01 degrees, cut into a 6 x 4 grid of
6,144 x 4,608-cell shards — 24 objects per frame, nine frames per cycle. One shard is 28.3 M cells,
94 % of §1's 30 M ceiling, and its 24 x 18 = 432 tiles of edge 256 page at 128 entries to
`4 x 1,540 = 6,160` directory bytes, so the header-plus-first-page fetch a corridor starts with is
under 1.6 KiB against a shard of tens of megabytes of raw cells.

The payload is what the tile codec decides. Worst-case raw4 would be `432 x 32,768 = 14.2 MB` per
shard, but real frames are dominated by all-dry sentinels and, for the tiles that carry weather, by
§5's deflate4 payloads: WXR1 measured a whole **wet global cycle** — 216 objects — at **14.69 MB**
with codec 2 against 43.60 MB without it. Upsampled coarse data is where the codec earns most: a
27.75 km floor cell paints a 3 x 3-ish block of identical lattice cells, which RLE4 cannot express
across rows and deflate4 collapses in both axes, up to 42x on the measured tiles.

The tile edge is the one number that trades against the corridor. At edge 64 the same cycle is
43.60 MB published, and a 90 km corridor over-fetches less; at edge 256 the phone's inflate-and-scan
is *faster* despite a 6.4x over-fetch, because it is fewer, larger, better-compressed reads. 256 is
what the publisher emits.
