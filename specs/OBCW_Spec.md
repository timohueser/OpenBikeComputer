# OBCW v1 weather bundle specification

Status: **normative** for format version 1. The Rust byte authority is
`firmware/obc-formats/src/obcw.rs`; the allocation-free traversal is
`firmware/obc-weather`. The Swift implementation is an independent consumer of this document. A
producer in another language must not need any implementation.

OBCW is the provider-neutral object delivered by a companion to OpenBikeComputer. It contains
exactly 24 hourly conditions and zero or more genuine precipitation-grid frames. HTTP products,
provider identifiers, projections, licensing metadata and selection policy end at the phone.
Firmware sees only this file.

The words MUST, MUST NOT, SHOULD and MAY are normative. All multibyte integers are **little
endian**. All offsets are absolute from byte zero. Integers are unsigned unless their type begins
with `int`. Unix timestamps are signed 64-bit seconds since 1970-01-01T00:00:00Z; leap seconds are
not represented. Coordinates are signed integer microdegrees (10^-6 degrees). Strings and floats
do not occur anywhere in v1.

## 1. Design and limits

- Format offsets and lengths are `uint32`. A reader MUST use checked addition and multiplication
  and MUST reject a value it cannot represent or address.
- A **phone producer policy**, separate from the format, caps v1 objects at 262,144 bytes. A
  conforming reader MUST NOT treat 262,144 as a format limit. Future transports may carry a larger
  valid v1 object without changing the byte layout. The number was 65,536 until WXR5 (#1244): under
  one uniform 1 km dataset the phone's corridor is 162 x 162 cells in every frame, whose raw4 worst
  case is ~153.6 kB, and the policy was raised to hold it with headroom. Nothing about the byte
  layout moved, and the device's reader is a windowed streamer whose resident bytes do not depend
  on object size — what a bigger object costs is transfer time.
- Rain tiles are fixed at 16 x 16 cells and independently addressable. A reader needs one
  128-byte encoded-tile buffer and one caller-owned 256-byte decoded-tile buffer; it never needs a
  whole frame in RAM.
- Providers may supply different grid dimensions, cell sizes and frame counts. A typical DWD
  producer uses a 96 x 96 grid at nominal 1,000 m and nine genuine frames at 0, +15, ...,+120
  minutes. A coarse global model emits only its genuine timestamps and resolution.
- V1 grids do not cross the antimeridian. A producer whose source window does must split/select a
  non-crossing window before encoding.
- No provider IDs, source URLs, strings, polygons, contours, mipmaps, colors or screen bitmaps are
  permitted. Provider attribution and diagnostics stay on the companion.

## 2. Canonical file order

A v1 file has exactly this order, with no gaps:

1. 112-byte header;
2. 24 x 24-byte hourly records;
3. `frame_count` x 48-byte rain-frame descriptors;
4. for each frame in descriptor order: its tile directory followed immediately by its tile data.

Offsets restate that layout so truncation and aliases are detectable. A reader MUST require every
offset to equal the checked end of the preceding region. Thus header, hourly section, descriptor
section, tile directories and payloads can never overlap. The checked end of the final tile
payload MUST equal `total_len`; trailing bytes are invalid.

This canonical order is intentional. It lets a low-RAM reader prove non-overlap in a single pass
without retaining every interval.

## 3. Header (112 bytes)

| Offset | Field | Size | Type | V1 rule |
| ---: | --- | ---: | --- | --- |
| 0 | Magic | 4 | `uint8[4]` | ASCII `OBCW` |
| 4 | Version | 2 | `uint16` | `1` |
| 6 | Header Len | 2 | `uint16` | `112` |
| 8 | Total Len | 4 | `uint32` | Exact file length |
| 12 | Generation | 4 | `uint32` | Monotonic cache generation chosen by the producer |
| 16 | Request ID | 4 | `uint32` | Echoes the device request; `0` only for unsolicited/manual material |
| 20 | Generated At | 8 | `int64` | Time the normalized bundle was built; positive |
| 28 | Valid From | 8 | `int64` | Hourly base: the beginning of hourly record 0 |
| 36 | Valid Until | 8 | `int64` | Overall validity ceiling; every hourly interval end and rain `valid_at` is `<=` this value |
| 44 | South Latitude | 4 | `int32` | Overall grid bound, -90,000,000...90,000,000 |
| 48 | West Longitude | 4 | `int32` | Overall grid bound, -180,000,000...180,000,000 |
| 52 | North Latitude | 4 | `int32` | Strictly greater than south |
| 56 | East Longitude | 4 | `int32` | Strictly greater than west; antimeridian crossing is not v1 |
| 60 | Grid Origin Latitude | 4 | `int32` | South edge reference for cell `(row=0,col=0)`; equals South Latitude in v1 |
| 64 | Grid Origin Longitude | 4 | `int32` | West edge reference for cell `(0,0)`; equals West Longitude in v1 |
| 68 | Hourly Offset | 4 | `uint32` | `112` |
| 72 | Hourly Count | 2 | `uint16` | `24` |
| 74 | Hourly Record Len | 2 | `uint16` | `24` |
| 76 | Frame Directory Offset | 4 | `uint32` | `112 + 24 x 24 = 688` |
| 80 | Frame Count | 2 | `uint16` | Number of genuine precipitation timestamps; zero is legal |
| 82 | Frame Descriptor Len | 2 | `uint16` | `48` |
| 84 | Bundle Flags | 4 | `uint32` | `0`; all v1 bits reserved |
| 88 | Bundle CRC-32 | 4 | `uint32` | Section 8 |
| 92 | Reserved | 20 | - | All zero |

Bounds describe the common geographic window as half-open `[south,north) x [west,east)`. Frames
may change resolution and dimensions while retaining that window and origin. `cell_size_m` is the
source's nominal ground resolution for truthful UI/selection; it does not ask firmware to recreate
a provider projection.

## 4. Hourly section

There are exactly 24 fixed-width records for the 24 consecutive hours beginning at
`header.valid_from`. Record index `i` MUST have `valid_time_offset_s = i * 3600`. Its `valid_at` is
`header.valid_from + valid_time_offset_s`, computed with checked arithmetic, and is the beginning
of the represented hour. The interval is **`[valid_at, valid_at + 3600)`**. Its checked exclusive
end MUST be no later than `valid_until`; thus the last record requires
`valid_until >= valid_from + 24 * 3600`.

| Offset | Field | Size | Type | Unit / rule |
| ---: | --- | ---: | --- | --- |
| 0 | Valid Time Offset | 4 | `uint32` | Seconds after header `valid_from` |
| 4 | Temperature | 2 | `int16` | 0.1 degrees C; -1000...700, or `INT16_MIN` unavailable |
| 6 | Precipitation Amount | 2 | `uint16` | 0.1 mm during this hourly period, or `65535` unavailable |
| 8 | Precipitation Probability | 1 | `uint8` | 0...100 percent, or `255` unavailable |
| 9 | Condition | 1 | `uint8` | Canonical table below |
| 10 | Wind From | 2 | `uint16` | Meteorological degrees clockwise from true north, 0...359, or `65535` unavailable |
| 12 | Wind Speed | 2 | `uint16` | 0.1 m/s, 0...2000, or `65535` unavailable |
| 14 | Wind Gust | 2 | `uint16` | 0.1 m/s, 0...2000, or `65535` unavailable |
| 16 | Flags | 2 | `uint16` | `0`; all v1 bits reserved |
| 18 | Reserved | 6 | - | All zero |

Precipitation Amount is the total accumulated during that following-hour interval. Precipitation
Probability is the probability of precipitation during that same interval. Adapters whose source
labels an accumulation by its ending timestamp MUST therefore subtract the source interval before
assigning the OBCW record; they MUST NOT shift a preceding-hour amount onto `valid_at`. Other
fields describe conditions valid at the interval beginning. Missing values remain missing. In
particular, an unavailable precipitation amount/probability MUST NOT be normalized to zero.

### 4.1 Canonical conditions

Adapters map their source taxonomy to these weather semantics. Codes express conditions, not
provider products or icon names.

| Code | Meaning |
| ---: | --- |
| 0 | Clear |
| 1 | Mostly clear |
| 2 | Partly cloudy |
| 3 | Overcast |
| 4 | Fog |
| 5 | Drizzle |
| 6 | Rain |
| 7 | Sleet / mixed rain and snow |
| 8 | Snow |
| 9 | Showers |
| 10 | Thunderstorm |
| 11 | Hail |
| 12 | Wind as the primary condition |
| 13...254 | Reserved; reject |
| 255 | Condition unavailable |

Thunderstorm takes precedence over its associated precipitation; hail takes precedence when hail
is the hazard supplied by the source. UI icon/color selection is not part of OBCW.

## 5. Rain-frame descriptor (48 bytes)

Descriptors are sorted by strictly increasing `valid_at`. Each timestamp is the actual native
frame time, not an ordinal or forecast-step guess. It MUST be positive and no later than
`header.valid_until`. It MAY precede `header.valid_from`: `valid_from` is the hourly base, not the
earliest rain timestamp. This is required for a genuine latent observation such as an IMERG Early
frame from about four hours before the current-to-+24-hour hourly forecast.

V1 deliberately has no mutable maximum-age rule in its byte-validity contract. `valid_until` is
the one on-wire overall upper ceiling; producer/client freshness and coverage policy decides
whether an older observed frame is usable. Accepting a structurally valid pre-hourly-base frame
MUST NOT make stale or incomplete rain eligible for a dry claim, alert, or alert-clear signal.

| Offset | Field | Size | Type | V1 rule |
| ---: | --- | ---: | --- | --- |
| 0 | Valid At | 8 | `int64` | Positive actual frame validity timestamp, `<= header.valid_until`; may precede `valid_from` |
| 8 | Width | 2 | `uint16` | Cells west-to-east; nonzero |
| 10 | Height | 2 | `uint16` | Cells south-to-north; nonzero |
| 12 | Cell Size | 2 | `uint16` | Nominal metres, nonzero |
| 14 | Tile Edge | 1 | `uint8` | `16` |
| 15 | Reserved | 1 | - | Zero |
| 16 | Tile Directory Offset | 4 | `uint32` | Exact checked end of preceding region |
| 20 | Tile Count | 4 | `uint32` | `ceil(width/16) x ceil(height/16)` |
| 24 | Tile Data Offset | 4 | `uint32` | Directory offset + tile count x 12 |
| 28 | Tile Data Len | 4 | `uint32` | Sum of this frame's encoded tile lengths |
| 32 | Quality Flags | 4 | `uint32` | Semantic flags below |
| 36 | Reserved | 12 | - | All zero |

Rows advance north and columns advance east. Tiles are row-major over
`ceil(height/16) x ceil(width/16)`. Cells within a tile are row-major. A partial tile at the north
or east edge still decodes to 256 cells; cells outside the declared width/height MUST be the
no-data intensity. A consumer MUST clip those padding cells.

For an in-bounds coordinate, lookup is integer affine mapping over the common bounds:

```text
row = floor((lat - south) * height / (north - south))
col = floor((lon - west)  * width  / (east - west))
```

Intermediates MUST be checked signed 64-bit (or wider). The north/east edges are outside the
half-open window; drawing code may clip a pixel at that edge to the last cell, but data queries MUST
not claim coverage outside it.

That same split governs interpolation:

- **Data queries MUST sample nearest-neighbour**, using the selected cell exactly, with no
  interpolation and no fabricated sub-cell precision. A data query is anything that answers *"what
  is the intensity at this position"* for a purpose other than colouring a pixel: the intensity
  lookup a snapshot records, corridor and dry-claim walks, alert thresholds and their clears. The
  value such a query returns MUST be a value some single cell of the product actually holds.
- **Display MAY interpolate** between cells for legibility, and MAY therefore paint an intensity
  band that no cell reports.
- **No claim, alert, alert-clear or dry decision may derive from an interpolated value.** Rendering
  MUST NOT be able to change what the rider is *told*, only what they are *shown*.

Rationale (2026-08-10): 1 km products render as visibly hard squares, and the original blanket
prohibition on interpolation was written to protect the honesty of the *claims* — a device must
never report rain, or report none, on the strength of a number it invented. Confining the
prohibition to data queries keeps that protection intact while letting the raster be legible. The
reference implementation ships bilinear display sampling with nearest-neighbour queries, and pins
the separation with a test that runs the whole decision path under every display sampling mode.

### 5.1 Semantic quality flags

| Bit | Name | Meaning |
| ---: | --- | --- |
| 0 | Observed | Based primarily on an observation valid at this time |
| 1 | Forecast | Based primarily on a model/nowcast forecast valid at this time |
| 2 | Partial coverage | Some in-bounds cells are unavailable |
| 3 | Degraded | Producer used a valid but reduced-quality source/result |
| 4...31 | Reserved | Must be zero in v1 |

Exactly one of Observed or Forecast MUST be set. These flags say what the data means; they MUST
NOT identify DWD, NOAA or another provider.

**How the phone producer decides Observed, since WXR5 (#1244).** A producer policy, not a reader
rule — a reader takes the flag as given — but it is written down here because two independent
producers implement it and they disagreed once already. Under one mosaic dataset a frame is radar
over the rider and model fill across the seam at the same instant, so no rule about a frame's
*content* can be true of all of it. The flag therefore follows the frame's position in the timeline:
the frame at offset 0 whose validity is within the dataset's stated source skew is the analysis and
sets **Observed**; every forward frame sets **Forecast**. Dryness is not part of it in either
direction — an all-dry radar scan is an observation, and an all-dry forecast frame is not one.

**Cross-reference: this is deliberately looser than `OBCG_Spec.md` §3.2's rule, and the divergence
is safe in one direction.** That spec's publisher may set Observed only when `offset_min` is 0 *and*
every cell of the object came from an observation upstream — a checkable property, because an OBCG
object is one shard. The rule here is positional only, because the unit is an assembled frame that
cannot be uniform: content that is true of the shard over the rider is false of the shard across the
seam. Both specs keep the offset-0 condition, which is the one that prevents the actual failure —
an observation re-stamped at a future validity. What differs is that an OBCW frame may say Observed
over cells whose OBCG shard said Forecast, never the reverse, and that is bounded by §5's standing
rule above: no claim, alert, alert-clear or dry decision may derive from it. Neither spec should be
edited to match the other.

Partial coverage is likewise decided over the **assembled** frame: a cell no source object and no
measured-dry region reached. A producer composing one frame from several objects must not raise it
merely because one of those objects stopped at its own edge, where a neighbouring object supplies
exactly the cells it was missing.

## 6. Tile directory and payloads

Each tile-directory entry is 12 bytes:

| Offset | Field | Size | Type | V1 rule |
| ---: | --- | ---: | --- | --- |
| 0 | Data Offset | 4 | `uint32` | Absolute offset; first equals descriptor `tile_data_offset`, each next equals prior offset + length |
| 4 | Encoded Len | 2 | `uint16` | Exact payload length |
| 6 | Decoded Cells | 2 | `uint16` | `256` |
| 8 | Codec | 1 | `uint8` | `0` raw4 or `1` RLE4 |
| 9 | Flags | 1 | `uint8` | `0`; reserved |
| 10 | Reserved | 2 | - | Zero |

The checked end of the last payload MUST equal `tile_data_offset + tile_data_len`. A payload may
not alias its directory, another tile, another frame or a header section.

### 6.1 raw4 (codec 0)

Length is exactly 128 bytes. Each byte holds two row-major cells: the earlier cell is the low
nibble and the later cell is the high nibble. A raw4 tile is canonical only when its maximal-run
RLE4 encoding would be 128 bytes or longer. A decoder MUST reject raw4 when that RLE4 encoding
would be shorter than 128 bytes.

### 6.2 RLE4 (codec 1)

Each byte is one run. The high nibble stores `run_length - 1` (therefore 1...16 cells); the low
nibble stores the intensity. Runs never cross a tile boundary. Decoding MUST produce exactly 256
cells: fewer is truncated; more is an overlong/zip-bomb-style input and MUST be rejected before
writing beyond the caller's tile buffer.

Runs MUST be maximal subject to the 16-cell field limit. Two adjacent runs with the same intensity
are valid only when the first run has length 16, because only that full run forces a continuation.
For example, 20 equal cells encode as lengths 16 then 4; lengths 8 then 12 are noncanonical and
MUST be rejected. Encoders coalesce equal cells and split only at 16-cell boundaries.

An RLE4 payload MUST be shorter than 128 bytes. Producers MUST choose RLE4 if and only if its
maximal-run encoding is strictly smaller than raw4; ties use raw4. Decoders MUST reject the wrong
codec choice in either direction. This makes the representation deterministic.

## 7. Canonical 4-bit precipitation intensity

The table represents instantaneous/rate precipitation in millimetres per hour. Interval notation
is exact: a value on a lower bound belongs to that row. Producers should quantize from their best
non-negative rate; negative or non-finite source values are no-data. The provider-neutral Rust
authority is `obc-formats::precip4`; both OBCW and OBCG reuse its thresholds and its two canonical
tile codecs. Swift reuses `OBCPrecipitationTileCodec` within `OBCWeatherWire` for the same reason.
§6's codec column is a **closed two-value set** here and stays one: OBCG adds a third codec of its
own (`OBCG_Spec.md` §5, deflate over the raw4 nibbles) *above* the shared authority, so it cannot
appear in an OBCW tile and the device never needs a decompressor to read one.

| Code | Rate in mm/h | Meaning |
| ---: | --- | --- |
| 0 | exactly 0 | Dry; transparent in a rain overlay |
| 1 | (0, 0.10) | Trace |
| 2 | [0.10, 0.25) | Very light |
| 3 | [0.25, 0.50) | Light |
| 4 | [0.50, 1.00) | Light-moderate |
| 5 | [1.00, 2.00) | Moderate |
| 6 | [2.00, 4.00) | Moderate-heavy |
| 7 | [4.00, 6.00) | Heavy |
| 8 | [6.00, 10.00) | Very heavy |
| 9 | [10.00, 16.00) | Intense |
| 10 | [16.00, 25.00) | Severe |
| 11 | [25.00, 50.00) | Extreme |
| 12 | [50.00, infinity) | Exceptional |
| 13, 14 | - | Reserved; a reader MUST reject the tile |
| 15 | unavailable | No data; never dry and never an alert-clear signal |

Colors, opacity, dithering and alert thresholds are consumer policy and are deliberately absent.

## 8. Whole-bundle CRC

`bundle_crc32` is CRC-32/IEEE (reflected polynomial `0xEDB88320`, initialization and xor-out
`0xFFFFFFFF`; check value `CRC32("123456789") = 0xCBF43926`) over exactly `total_len` bytes while
bytes 88...91 are treated as four zeros. Writers zero the field, hash the finished bundle, then
store the result little-endian. Readers MUST validate it before accepting the object.

The transport may also carry an outer whole-object CRC. The internal CRC is still required: it
travels with cached OBCW bytes and protects parsing after transport metadata is gone.

## 9. Required validation order

A decoder MUST never panic, read outside the announced object or write beyond a tile buffer.
Equivalent early-exit ordering is allowed, but acceptance requires every check below:

1. Read the fixed header; validate magic, version, header length and reserved bytes.
2. Require `total_len` to equal the available object length; checked arithmetic only.
3. Validate fixed counts/record lengths, time range, coordinate bounds/origin and header flags.
4. Verify the internal CRC with the CRC field treated as zero.
5. Validate the 24 exact following-hour offsets, each interval end, fields, flags and reserved
   bytes.
6. Validate descriptor extent and canonical section order.
7. For each frame, validate a positive, strictly increasing timestamp no later than
   `valid_until`, plus dimensions, quality flags and computed tile count. Do not require a frame
   timestamp to be on or after the hourly base.
8. For each tile entry, validate canonical offset, lengths, reserved fields and codec.
9. Validate every raw nibble or RLE run, including maximal-run canonicality and the canonical
   raw4/RLE4 codec choice. Stop as soon as an RLE sum exceeds 256; require exactly 256 at its end.
10. Require the final checked end to equal `total_len`.

CRC success never excuses structural validation. A malicious producer can compute a CRC over
malformed data.

## 10. Golden and negative material

Checked-in files live in `specs/vectors/` and are described in its `manifest.json` and README.
Positive vectors cover a dry hourly-only bundle, raw and compressed tiles, a no-data tile, a
four-hour-latent observation with a current hourly base, a coarse-model shape, a 96 x 96 x
nine-frame DWD-shaped raw bundle, and the exact 262,144-byte producer-policy boundary. Negative
vectors isolate truncation, bad offsets, overlapping sections, nonzero hourly flags/reserved
bytes, reserved intensity nibbles, compressible raw4, overlong and noncanonical RLE, CRC mismatch,
timestamp disorder, a nonpositive frame time and a frame beyond the overall ceiling.

Rust and Swift must decode the same positives to the same values and independently re-encode the
exact bytes. Both must reject every negative. Vector provenance is recorded beside the fixtures.

## 11. Worked size budget

A 96 x 96 frame has `6 x 6 = 36` tiles. Forced raw4 costs `36 x (12 + 128) = 5,040` bytes per
frame. Nine frames cost 45,360 bytes. Header + hourly + descriptors cost
`112 + 24 x 24 + 9 x 48 = 1,120` bytes, for **46,480 bytes (45.39 KiB)** total. Real RLE4 can only
reduce it. This is the locked approximately 44-46 KiB DWD-shaped estimate.

The shape a phone actually produces since WXR5 (#1244) is bigger and no longer provider-shaped: one
uniform dataset, a 90 km corridor, and a uniform ~1,113 m cell pitch on both axes give **162 x 162
cells** in every frame at every latitude. That is `11 x 11 = 121` tiles, so forced raw4 costs
`121 x (12 + 128) = 16,940` bytes per frame and nine frames plus the same 1,120-byte fixed part
come to **153,580 bytes (150.0 KiB)** — 41 % under the 256 KiB producer policy. A real corridor
measures well below it: the largest bundle #1254 measured over a 0-80 degree sweep was 55.5 kB.

The allocation-free Rust reader validates CRC in 512-byte chunks, hourly records four at a time,
and canonical tile directories/payloads in four-tile windows. Its largest explicit simultaneously
live validation scratch is 864 bytes. A counting `ByteSource` regression over the 46,480-byte
DWD-shaped vector pins `WeatherReader::open` at **269 `read_at` calls and 92,848 bytes read**, down
from 1,046 calls while remaining below the 2 KiB scratch budget. The byte total is just under twice
the object because integrity verification and structural validation are separate fail-closed
passes. A representative Thumb `WeatherReader` plus its generation-and-CRC-keyed lookup cache is
**472 bytes**. Cold random tile lookup is pinned at at most **3 logical reads and 5 touched
512-byte blocks** over every tile of the DWD-shaped vector; an exact tile-cache hit performs no
reads. These measured implementation budgets do not change the wire format.

The device's dual-slot publication and generation-selection rules are intentionally outside the
wire format and are specified in [`firmware/docs/WEATHER_STORAGE.md`](../firmware/docs/WEATHER_STORAGE.md).
