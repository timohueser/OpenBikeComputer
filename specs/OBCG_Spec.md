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
  WX2 4-bit intensity table and raw4/RLE4 codec (`OBCW_Spec.md` §6-§7), generalized from 256
  cells to `tile_edge^2` cells; `obc-formats::precip4` is the one shared authority.
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
| 56 | Cell Size | 2 | `uint16` | Nominal source ground resolution in metres; nonzero |
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
as OBCW. `cell_size_m` is the source's nominal ground resolution for truthful UI and selection;
the exact lattice is the microdegree strides.

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
| 6...254 | - | Reserved for future registry additions |
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
| 6 | Codec | 1 | `uint8` | `0` raw4 or `1` RLE4; `0` for a dry tile |
| 7 | Reserved | 1 | - | Zero |
| 8 | Tile CRC-32 | 4 | `uint32` | CRC-32 of the payload bytes; `0` for a dry tile |

**Dry sentinel.** `encoded_len == 0` declares every cell of the tile (including edge padding)
to be intensity `0` (dry). A dry entry has no payload bytes and its other fields MUST be zero —
a dry entry is exactly twelve zero bytes. The sentinel means **dry, never no-data**: a tile of
unavailable cells MUST be encoded (as RLE4 no-data runs), because missing data must never decode
as dry weather.

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

The payload encoding is the canonical WX2 codec of `OBCW_Spec.md` §6.1/§6.2 with the decoded
cell count `N = tile_edge^2` in place of 256:

- **raw4 (codec 0)**: exactly `N / 2` bytes; each byte holds two row-major cells, earlier cell
  in the low nibble. Valid only when the maximal-run RLE4 encoding would be `N / 2` bytes or
  longer.
- **RLE4 (codec 1)**: one byte per run; high nibble `run_length - 1` (1...16 cells), low nibble
  the intensity. Runs MUST be maximal subject to the 16-cell limit (equal adjacent runs only
  after a full 16-cell run), MUST NOT cross the tile boundary, and MUST sum to exactly `N`
  cells — a reader stops as soon as the sum exceeds `N`. The payload MUST be shorter than
  `N / 2` bytes.

Producers MUST choose RLE4 if and only if its maximal-run encoding is strictly smaller than
raw4; ties use raw4. A tile whose `N` cells are all dry MUST use the §4.1 sentinel instead of a
payload; consequently a decoded payload with every cell dry is noncanonical and MUST be
rejected. (An edge tile can never be all-dry, because its padding is no-data.)

Cell intensities are the canonical 4-bit precipitation table of `OBCW_Spec.md` §7 — the same
codes, the same mm/h thresholds, the same reserved values 13/14 (reject) and no-data 15 (never
dry, never an alert-clear signal). OBCG adds no second quantization authority.

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
half-open window. Nearest-neighbour sampling uses the selected cell exactly; no bilinear
interpolation or fabricated sub-cell precision is permitted, matching the epic's no-smoothing
rule end to end.

## 7. Corridor extraction

A corridor consumer performs, in order:

1. **Header read**: the first 128 bytes (any first read `<= 16 KiB` is conforming; 128 bytes
   suffice). Validate §3 including the header CRC before trusting any derived arithmetic.
2. **Directory page reads**: compute the tile index range covering the corridor (§6), map the
   needed indexes to pages (§4), and fetch exactly those pages. Validate each page's CRC and the
   §4.1 entry rules for the entries it uses.
3. **Tile reads**: fetch `[data_offset, data_offset + encoded_len)` for each needed non-dry
   entry, validate the tile CRC, then decode under the §5 rules. Dry-sentinel tiles cost no
   read.

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
6. For each tile payload used: verify the tile CRC, then the §5 codec rules including reserved
   intensities, maximal runs, exact cell count, canonical codec choice, the all-dry
   noncanonicality rule, and no-data edge padding.

## 10. Service manifest

The manifest is the mutable JSON document `wx/v1/manifest.json` beside the immutable frame
objects; its JSON Schema is checked in at `host/obc-wx-bake/schema/manifest.schema.json` and
pinned by tests. It is delivery metadata, not a byte format, but its contract is normative:

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

## 11. Golden and negative material

Checked-in files live in `specs/vectors/` (`grid-*.obcg`) and are described in its
`manifest.json` and README. Positive vectors cover the all-dry sentinel object, raw4 and RLE4
tiles, an explicit all-no-data tile, edge-tile no-data padding, and a multi-page directory with
last-page padding. Negative vectors isolate truncation, bad payload offsets, overlapping/
non-canonically packed payloads, impossible dimensions, a non-power-of-two tile edge, bad paging
parameters, overlong RLE, a compressible raw4 tile, a noncanonical encoded all-dry tile, a
nonzero dry sentinel, header CRC, object CRC, page CRC and tile CRC mismatches, and nonzero
reserved bytes.

Rust builds and validates them through the `obc-formats` authority; Swift independently decodes
the same positives to the same cells and rejects every negative. Vector provenance is recorded
beside the fixtures. Byte or layout changes to this specification MUST land with updated
vectors and both implementations in the same change, per the epic's working agreements.

## 12. Worked size budget

The launch DWD RV product window is 1,234 x 1,132 cells (§3 strides 9,000 x 14,000 udeg at
nominal 1,000 m). At tile edge 32 that is `39 x 36 = 1,404` tiles; with 512 entries per page the
directory is `3 x 6,148 = 18,444` bytes and the header-plus-one-page first fetch is under
6.3 KiB. A dry Germany frame is `128 + 18,444 = 18,572` bytes total. Worst-case raw4 payloads
would add `1,404 x 512 = 718,848` bytes, but real frames are dominated by dry sentinels and
short RLE4 runs. ICON-EU (1,377 x 657 at 0.0625 deg, tile edge 16, `87 x 42 = 3,654` tiles)
pages to `8 x 6,148 = 49,184` directory bytes. A CONUS-scale MRMS frame (7,000 x 3,500, tile
edge 64) is `110 x 55 = 6,050` tiles — twelve pages, 73,776 directory bytes — which is exactly
why the directory is paged
rather than "one small read": at tile edge 16 it would be 95,922 entries.
