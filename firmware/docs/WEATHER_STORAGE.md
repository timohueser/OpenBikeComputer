# Weather storage architecture

This document fixes the device-side storage and selection policy for OBCW v1 bundles. The wire
format remains normative in [`specs/OBCW_Spec.md`](../../specs/OBCW_Spec.md); this policy is about
how validated objects are published and read from a microSD card.

## Boundaries

- `obc-formats` owns the OBCW byte layout.
- `obc-weather` owns allocation-free validation, timestamp/geographic lookup, the fixed lookup
  cache, and pure dual-slot selection. It has no filesystem or transport dependency.
- `obc-storage::weather` owns the transport-independent publication state machine through the
  `WeatherSlotIo` trait.
- `obc-fw-nrf54l::sd` and `obc-sim::weather_store` are filesystem adapters. Both hand stable
  `ByteSource`s to `obc-weather`, so simulator and firmware use exactly the same validator and
  selector.

Provider clients, phone scheduling, BLE object routing, weather UI, and rendering are outside
these layers. In particular, adding another provider does not change firmware storage.

## Fixed slots and boot selection

The card root contains exactly two eligible names: `WEATHER.A` and `WEATHER.B`. There is no
weather `UPLOAD.TMP`, rename chain, or scan for alternate filenames. Boot fully validates both
slots through `WeatherReader::open`; missing, unreadable, truncated, CRC-invalid, or structurally
invalid files are never candidates.

When both slots are valid, generation comparison uses wrapping RFC-1982-style serial arithmetic:

- a non-zero difference other than `2^31` has an unambiguous serial-newer value;
- equal generations use later `generated_at`, then slot A for an exact tie;
- the exactly-`2^31` ambiguous difference also uses later `generated_at`, then slot A for an exact
  tie.

Publication is stricter than boot tie-breaking: an incoming equal/half-range generation replaces
the active slot only when its `generated_at` is later. Exact ties are rejected as not newer.

## Crash-safe publication

`WeatherUpload` receives the announced object length and an outer transport CRC, then performs
this sequence:

1. Fully inspect both slots and choose only a proven-safe inactive slot. An unreadable slot is not
   truncated because it might contain the only valid object.
2. Truncate/create that exact inactive name and write four zero bytes at offset zero.
3. Hold the incoming `OBCW` magic in memory, stream only bytes `4..`, and update the outer CRC.
4. Flush and close the inactive body.
5. Overlay the held magic while re-opening the closed file through the complete OBCW validator.
   This proves its internal CRC, canonical layout, timestamps, directory entries, and every tile
   payload without first making it boot-eligible.
6. Reject a candidate that is not strictly newer than the active one.
7. Patch the real four-byte magic, flush, and close. This is the eligibility point.

Until step 7, any power cut leaves zero magic and boot ignores the partial slot. The old active
slot is never modified. If the final magic write or its flush reports an error, storage semantics
allow **either** of two legal media outcomes: the patch did not persist and boot selects the old
slot, or the complete patch persisted and boot may select the new slot. Both are correct recovery
outcomes because boot revalidates both complete objects and applies the deterministic selector.
The upload API reports the I/O error rather than claiming which persistence outcome occurred.

Transport abort, card-full, partial append, body-close failure, invalid outer/internal CRC, and
card removal leave the old active bytes untouched. A retry always begins from byte zero in a newly
selected inactive slot.

## Reader and I/O budgets

`WeatherReader` never allocates a whole object or frame. `WeatherCache` retains one frame
descriptor, a four-entry tile-directory window, and one decoded 16 x 16 tile. Every cache key uses
both generation and validated bundle CRC, so a later object cannot inherit bytes from an
equal-generation predecessor.

The Thumb target reports a representative reader plus cache at **472 bytes**. Compile-time checks
keep that total below the 2 KiB target and hard 4 KiB ceiling. The 46,480-byte DWD-shaped fixture
pins a cold random tile lookup at at most **3 logical `read_at` calls** and **5 touched 512-byte
blocks**; an exact tile hit performs no reads, and another tile in the same four-entry directory
window needs only its payload read. Tests exhaust all 324 tiles in the nine-frame fixture.

These are logical source/block ceilings, not claims about a filesystem driver's internal cache or
physical card command latency.
