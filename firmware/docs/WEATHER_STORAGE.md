# Weather storage architecture

This document fixes the device-side storage and selection policy for OBCW v1 bundles. The wire
format remains normative in [`specs/OBCW_Spec.md`](../../specs/OBCW_Spec.md); this policy describes
how validated weather objects are published and read from a flat-store card.

## Boundaries

- `obc-formats` owns the OBCW byte layout.
- `obc-weather` owns allocation-free validation, timestamp/geographic lookup and the fixed lookup
  cache. It has no card-format or transport dependency.
- `obc-storage::flat` owns immutable object revisions and atomic catalog commits.
- `obc-link` carries protocol-v4 `PUT`/`GET`/`LIST`/`REMOVE` records without parsing OBCW.
- `obc-fw-nrf54l::flat_store` selects, validates and holds the active kind-4 object for the ride and
  weather scheduler planes.

Provider clients, phone scheduling, weather UI and rendering stay outside those layers. Adding
another provider does not change device storage.

## Active object selection

Every non-`RETAINED` `WeatherBundle` catalog head is a candidate. The board fully validates each
candidate with `WeatherReader::open`; a truncated, CRC-invalid, structurally invalid or unreadable
object is omitted.

When multiple valid object ids exist, the OBCW generation is compared with wrapping
RFC-1982-style serial arithmetic. Equal generations and the exactly-`2^31` ambiguous difference
use the later `generated_at`; an exact OBCW identity tie uses the larger flat-store `ObjectId` for a
stable result. The selected identity includes `(ObjectId, Revision)`, so replacing an object always
invalidates the reader even if a producer repeats generation metadata.

## Crash-safe publication

The protocol-v4 engine reserves fresh extents, writes the complete payload, verifies the transfer
CRC and atomically commits the new catalog entry. A replacement can mark the displaced weather
revision `RETAINED` in that same commit. Until the commit, readers continue to resolve the old
head; after it, new opens resolve the new immutable revision. An interrupted transfer is never a
catalog entry and cannot become active after reboot.

The board validates OBCW after publication at the domain load seam. A well-formed transfer carrying
the wrong bytes can therefore occupy an object revision but cannot become the active weather
bundle. Removing or replacing it uses the ordinary flat-store operations; there are no held-magic
files, inactive slots, temporary names or filesystem rename rules.

## Reader and scheduler lifecycle

`FlatWeather` retains the validated header proof beside the selected `(ObjectId, Revision)`. The
ride plane holds that exact immutable revision open and reconstructs a reader with one matching
header read for dashboard sampling and Rain Map rendering. A catalog movement revalidates the
selection, releases the old hold and opens the new revision between synchronous reader uses.

The due scheduler also wakes on catalog movement. It completes an outstanding request only when a
new validated weather head contains that exact request id. Route, trip, map, delete and malformed
weather commits therefore cannot falsely finish the retry ladder. A valid unrelated weather object
can still become readable, but it does not acknowledge another request.

## Reader and I/O budgets

`WeatherReader` never allocates a whole object or frame. `WeatherCache` retains one frame
descriptor, a four-entry tile-directory window and one decoded 16 x 16 tile. Every cache key uses
both generation and validated bundle CRC, so a later object cannot inherit bytes from an
equal-generation predecessor.

The Thumb target reports a representative reader plus cache at **472 bytes**. Compile-time checks
keep that total below the 2 KiB target and hard 4 KiB ceiling. The 46,480-byte DWD-shaped fixture
pins a cold random tile lookup at at most **3 logical `read_at` calls** and **5 touched 512-byte
blocks**; an exact tile hit performs no reads, and another tile in the same four-entry directory
window needs only its payload read. Tests exhaust all 324 tiles in the nine-frame fixture.

These are logical source/block ceilings, not claims about card-command latency.
