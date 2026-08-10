# OBCG v1 published precipitation grid specification

Status: **normative** for format version 1. The Rust byte authority is
`firmware/obc-formats/src/obcg.rs`; the producer is `host/obc-wx-bake`. The Swift implementation
is an independent consumer of this document. A consumer in another language must not need any
implementation.

OBCG is the static object the OpenBikeComputer weather service publishes to object storage. One
OBCG object is exactly **one grid frame**: one product, one real UTC validity timestamp, one
regular latitude/longitude window at one native cell size. There is deliberately no in-object
multi-frame table — the frame set, its keys and its policy metadata live in the service manifest
(§10), so a product whose frames have heterogeneous geometry (a 1 km radar observation followed
by 3 km model forward frames) composes with **no resampling, by construction**.

> **Superseded 2026-08-10 by #1242; to be deleted by #1246.** The publisher no longer works this
> way. It normalises every source onto **one** canonical global 0.01 degree lattice at bake time
> — coarse sources cell-replicated, overlaps resolved per cell by a fixed priority table that is
> baker configuration and never client policy — and publishes one provider-agnostic dataset
> sharded across objects. So "no resampling, by construction" describes the shape this format was
> designed around, not the shape it now carries: the resampling happens once, in the baker,
> nearest-neighbour as §6 has always required, instead of being pushed onto every consumer as a
> selection policy. The nesting obligation in the next paragraph and the product/selection model
> in §10 are superseded with it — a single-lattice dataset trivially satisfies both — and #1246
> removes all three together with the multi-product path. Nothing about the *bytes* changes; this
> is framing prose only.

That composition carries one normative obligation on the *publisher*. A consumer assembling a
multi-frame bundle (OBCW) states one geographic window for the whole timeline and takes it from
the coarsest frame; every other frame is laid onto that window at its own cell size, and a frame
the window cannot tile exactly is **refused, never resampled**. A publisher MUST therefore ensure
that within one product, every frame's lattice nests under every coarser frame's: each coarser
frame's `cell_lat_udeg` and `cell_lon_udeg` MUST be integer multiples of the finer frame's, and
the two grid origins MUST be congruent modulo the finer frame's cell size in each axis. A product
violating this is not malformed OBCG — each object is individually valid — but consumers will
silently drop frames from it, and the frame most likely to be dropped is the finest one, which is
normally the radar observation. Publishers SHOULD verify this before publishing and fail the
cycle rather than emit such a product.

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
- The tile edge is a **per-product power of two** between 16 and 256 cells, chosen by the
  producer so directories stay small and tiles stay corridor-sized. Tiles reuse the canonical
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
  `<= +180 deg`. A worldwide source whose native window crosses it MUST be published as more
  than one product window (for example an eastern and a western object per timestamp); the
  manifest composes them.
- An OBCG object carries provenance (product id, tier, source class, reference time), never
  policy. Selection, staleness and attribution are manifest data; a consumer MUST NOT branch
  behavior on the in-object product id and MUST NOT reject an unknown nonzero product id.

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
| 12 | Product ID | 1 | `uint8` | Nonzero registry code (§3.1) |
| 13 | Tier | 1 | `uint8` | Nonzero: `1` radar, `2` model, `3` floor |
| 14 | Flags | 2 | `uint16` | §3.2; exactly one source-class bit |
| 16 | Valid At | 8 | `int64` | Real upstream UTC frame validity time; positive |
| 24 | Reference Time | 8 | `int64` | Upstream run/reference UTC time; positive, `<= valid_at` |
| 32 | South Latitude | 4 | `int32` | South grid edge, `>= -90,000,000` |
| 36 | West Longitude | 4 | `int32` | West grid edge, `>= -180,000,000` |
| 40 | Cell Lat Stride | 4 | `uint32` | Microdegrees per cell northward; nonzero |
| 44 | Cell Lon Stride | 4 | `uint32` | Microdegrees per cell eastward; nonzero |
| 48 | Width | 4 | `uint32` | Cells west-to-east; §1 bounds |
| 52 | Height | 4 | `uint32` | Cells south-to-north; §1 bounds |
| 56 | Cell Size | 2 | `uint16` | The lattice's cell size in metres; nonzero |
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

> **Amended 2026-08-10 by #1242.** `cell_size_m` used to be "the source's nominal ground
> resolution for truthful UI and selection". Under the mosaic a frame has no single source
> resolution — its German cells come from 1 km radar and its Italian cells from 6.5 km model — so
> the field states the lattice instead, and the information is *removed* rather than transported:
> there is no per-cell resolution plane, no per-tile source label and no coverage channel. That is
> honest because the mosaic always has a global floor source, so every cell always carries a
> best-available value; "no radar coverage" renders as model fill, not as dry. Intensity code 15
> remains the only "we do not know", per §4.1. For the canonical 0.01 degree lattice the publisher
> emits `1113` — 0.01 degrees of latitude, in metres.

`entries_per_page <= 1365` keeps every directory page (and the header) inside one 16 KiB Range
request: `1365 x 12 + 4 = 16,384` bytes.

### 3.1 Product registry

The product id is provenance, mirroring the manifest's product id string. Appending a code to
this table is a spec-table addition, **not** a format version bump, because adding a weather
source must never require a firmware, protocol or app release. A consumer MUST NOT reject an
unknown nonzero code and MUST NOT use it for selection policy.

| Code | Manifest id | Source |
| ---: | --- | --- |
| 0 | - | Invalid; reject |
| 1 | `dwd-rv` | DWD RV composite (Germany radar nowcast) |
| 2 | `icon-eu` | DWD ICON-EU `TOT_PREC` (Europe model) |
| 3 | `mrms` | NOAA MRMS `PrecipRate` (CONUS radar observation) |
| 4 | `hrrr` | NOAA HRRR subhourly `PRATE` (CONUS model) |
| 5 | `gfs` | NOAA GFS `APCP` (worldwide floor) |
| 6 | (the one dataset) | Canonical mosaic: every source normalised onto the global lattice, best available per cell, no provenance carried (#1242) |
| 7...254 | - | Reserved for future registry additions |
| 255 | - | Experimental / private products |

### 3.2 Flags

| Bit | Name | Meaning |
| ---: | --- | --- |
| 0 | Observed | The frame is primarily an observation valid at `valid_at` |
| 1 | Forecast | The frame is primarily a model/nowcast forecast valid at `valid_at` |
| 2...15 | Reserved | Must be zero in v1 |

Exactly one of Observed and Forecast MUST be set. `valid_at` is always the genuine upstream
timestamp — a latent observation keeps its old timestamp, and a forecast lead is
`valid_at - reference_time`. Re-stamping fetch or bake time here is forbidden.

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
extending it is a format version bump, not a table addition (unlike §3.1's product registry,
which is provenance a consumer never branches on). The tile CRC covers the payload **as stored**,
so a consumer verifies integrity *before* decompressing anything (§9 step 6).

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

Rationale (2026-08-10): the epic's original rule was no smoothing anywhere, which made 1 km
products render as hard squares. The rule that was actually load-bearing — that a device never
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
2. Validate product id, tier, flags, timestamps, geometry bounds and §1 limits, tile edge and
   paging parameters, and the derived section layout (`directory_offset`, `data_offset`,
   `total_len`) with checked arithmetic only.
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

The manifest is the mutable JSON document `wx/v1/manifest.json` beside the immutable frame
objects; its JSON Schema is checked in at `host/obc-wx-bake/schema/manifest.schema.json` and
pinned by tests. It is delivery metadata, not a byte format, but its contract is normative:

> **Superseded 2026-08-10 by #1242; replaced by #1243, deleted by #1246.** The product/selection
> model below — `products[]`, tiers, per-product bboxes and the client-side choice between them —
> describes the multi-product service. The baker now publishes one dataset on one lattice, so
> there is nothing to select between. #1243 defines the manifest that replaces this section; until
> it lands, the canonical dataset publishes a placeholder document beside the `wx/v1` tree that no
> client reads.

- Frame objects are published under immutable keys
  `wx/v1/<product>/<generated-utc>/f<offset-min>.obcg`, where `<generated-utc>` is the upstream
  reference time (`YYYYMMDD'T'HHMM'Z'`) and `<offset-min>` is `(valid_at - reference_time) / 60`.
  Frames upload first; the manifest swaps last, atomically. A failed cycle leaves the previous
  manifest and its frames fully consistent.
- Frame objects carry long immutable `Cache-Control`; the manifest caches for at most 60 s.
- `products[]` entries carry: `id`, `tier`, product bbox and nominal cell size, `generated_at`,
  `staleness_deadline`, attribution (text + URL, from the WX1 license record), and `frames[]`.
- Each `frames[]` entry restates the frame's full geometry (bbox edges, strides, dimensions,
  tile edge, paging) plus `valid_at`, source class, key, byte length and the object CRC — so a
  client can plan corridor reads and verify integrity without trusting anything but the
  manifest and its own range reads, and heterogeneous per-frame geometry is first-class.
- A product bbox is the intersection of its frames' bboxes: the region where the whole timeline
  is answerable. Selection policy ("highest fresh tier covering the corridor") consumes the
  manifest only.
- `staleness_deadline` is the moment the product must stop being used if no fresh manifest has
  replaced it. Expired products keep their true timestamps; expiry makes them unusable, never
  silently dry.
- The manifest always lists everything the service currently offers, whichever cycle wrote it. A
  publishing cycle may cover a **subset** of the products — the deployed service runs one timer per
  adapter, so a broken upstream costs only its own product's freshness — and the products it did
  not bake are carried forward from the previous manifest verbatim. A client must never infer from
  a product's presence that this document's cycle refreshed it: the product's own `generated_at`
  is that fact, not the manifest's.
- **Expiry never removes an entry.** A product past its `staleness_deadline` stays listed with its
  true timestamps, so expiry is visible rather than silent. In exchange, a publisher must not
  require an expired product's frame objects to still exist — no client may read them, and a
  lifecycle rule is entitled to have collected them. Removing a product from the manifest is an
  operator act (retiring an adapter), never a consequence of an outage: a client may therefore
  read an absent product as "this service does not offer it", and a monitor may read it as a
  configuration fault rather than as weather.

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

Rust builds and validates them through the `obc-formats` authority; Swift independently decodes
the same positives to the same cells and rejects every negative. Vector provenance is recorded
beside the fixtures. Byte or layout changes to this specification MUST land with updated
vectors and both implementations in the same change, per the epic's working agreements.

## 12. Worked size budget

The launch DWD RV product window is 1,234 x 1,132 cells (§3 strides 9,000 x 14,000 udeg at
nominal 1,000 m). At tile edge 32 that is `39 x 36 = 1,404` tiles; with 512 entries per page the
directory is `3 x 6,148 = 18,444` bytes and the header-plus-one-page first fetch is under
6.3 KiB. A dry Germany frame is `24,440` bytes total: the `38 x 35 = 1,330` full interior tiles
are sentinels, while the 74 partial north/east edge tiles carry short dry-plus-no-data RLE4
payloads (`35 x 96 + 38 x 64 + 76 = 5,868` bytes) — the §4.1 edge rule keeps padding honest
even on a dry day. Worst-case raw4 payloads
would add `1,404 x 512 = 718,848` bytes, but real frames are dominated by dry sentinels and, for
the tiles that do carry weather, by §5's deflate4 payloads — WXR1 measured a whole wet global
cycle at 14.69 MB with codec 2 against 43.60 MB without it. ICON-EU (1,377 x 657 at 0.0625 deg, tile edge 16, `87 x 42 = 3,654` tiles)
pages to `8 x 6,148 = 49,184` directory bytes. A CONUS-scale MRMS frame (7,000 x 3,500, tile
edge 64) is `110 x 55 = 6,050` tiles — twelve pages, 73,776 directory bytes — which is exactly
why the directory is paged
rather than "one small read": at tile edge 16 it would be 95,922 entries.
