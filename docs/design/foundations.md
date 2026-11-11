---
system: foundations
kind: system
area: Device
purpose: "Holds the shared building blocks that have no product policy: the byte layouts of the file formats, the byte-reading interface, CRC-32, the host interfaces and the map-scene interface."
owns:
  - firmware/obc-formats/**
  - firmware/obc-crc/**
  - firmware/obc-ports/**
  - firmware/obc-map-scene/**
  - host/obc-file-source/**
depends_on: []
test_depends_on:
  - map-reading
status: draft
---

# Foundations

## What it does

This system holds the small crates that most other crates build on. They contain no product policy: no screens, no reader of a whole file, no storage driver and no executor. Each crate is `no_std` except the host crate `obc-file-source`, so the same code runs on the device, in the simulators and in the host tools.

There are five crates:

- [`obc-formats`](../../firmware/obc-formats/src/lib.rs) is the code copy of the byte contracts in [`specs/`](../../specs/README.md). It holds the fixed sizes, versions, flags, sentinel values and small record codecs (encode and decode functions) of the map (OBCM), route (OBCR), terrain (OBCT) and ride formats. It also holds the byte-reading interface `ByteSource` and the writing interface `ByteSink`, the little-endian read and write helpers, and the block cache that every quadtree walk reads through. The specs stay the contract. When the two differ, the code is wrong.
- [`obc-crc`](../../firmware/obc-crc/src/lib.rs) is the only CRC-32 in the firmware. The update image format and the bootloader (`obc-dfu`), and the BLE object transfer (`obc-ble`), both use it. The companion app has a Swift copy that must give the same result.
- [`obc-ports`](../../firmware/obc-ports/src/lib.rs) holds the values and the narrow synchronous traits that cross between the app core and a host: sensor samples and sensor sources, button input, track points, the settings store, the backlight, the sounder and power-off. Each board or simulator implements these traits where it builds the app.
- [`obc-map-scene`](../../firmware/obc-map-scene/src/lib.rs) is the interface between a map source and the renderer: bounds, styles, level-of-detail choice, a walk over the visible features, decode of the selected features, and read diagnostics. It knows nothing about OBCM byte offsets or storage. `obc-reader` implements it for OBCM; tests implement it for static scenes.
- [`obc-file-source`](../../host/obc-file-source/src/lib.rs) puts a file on disk behind `ByteSource`, for the host tools: the packer, the bakers, the assembler, the inspector and the simulators.

All readers take their bytes through `ByteSource`. It reads any byte range on request, so a map does not have to fit in RAM. The guide pages [system architecture](../content/software/architecture.md) and [data formats](../content/software/formats.md) explain why.

The system depends on no other system. Its tests use `obcm-testkit` from map reading for a temporary file path.

## Rules that must hold

| Rule | Check |
| --- | --- |
| The CRC is CRC-32/IEEE (the zlib one): its value for `"123456789"` is `0xCBF43926`. | `check_value` in [`lib.rs`](../../firmware/obc-crc/src/lib.rs) |
| A CRC computed in parts equals the CRC computed in one step, and a stored checksum can continue a CRC. | `incremental_matches_oneshot`, `final_checksum_resumes_without_rereading_the_prefix` in [`lib.rs`](../../firmware/obc-crc/src/lib.rs) |
| The format constants and codecs agree with the committed test vectors in `specs/vectors/`. | `constants_match_committed_route_fixtures` in [`obcr.rs`](../../firmware/obc-formats/src/obcr.rs), `committed_v6_vector_uses_the_production_footer_codec` in [`ride.rs`](../../firmware/obc-formats/src/ride.rs), `estimate_matches_the_shared_vector_file` in [`bike.rs`](../../firmware/obc-formats/src/bike.rs) |
| The navigation edge walk refuses a malformed record and every record after it in the same chunk. | `a_malformed_intermediate_record_refuses_the_ordinals_behind_it` in [`obcm.rs`](../../firmware/obc-formats/src/obcm.rs) |
| A failed cache fill leaves an empty slot, never a slot with wrong bytes. The caller decides whether a block enters the cache. | `a_torn_fill_leaves_an_empty_slot_not_a_poisoned_one`, `the_insertion_decision_comes_from_the_caller` in [`cache.rs`](../../firmware/obc-formats/src/cache.rs) |
| A trip start record never replaces a finished trip record. | `a_start_record_moves_the_stored_record_and_never_replaces_it` in [`trip_progress.rs`](../../firmware/obc-formats/src/trip_progress.rs) |
| A map style stays at 12 bytes or less, because a source keeps 256 of them in RAM. | `size_of::<Style>` in [`lib.rs`](../../firmware/obc-map-scene/src/lib.rs) (a `const` assert) |
| A file source refuses a read past the end of the file. | `reads_at_an_offset_and_refuses_the_end` in [`lib.rs`](../../host/obc-file-source/src/lib.rs) |

The `no_std` and `forbid(unsafe_code)` attributes in each crate root are checked by the compiler. No check stops a policy crate from being added as a dependency of these crates. (open: should `cairn check` or a guard refuse dependencies other than `heapless`, `libm` and `obc-ports`?)

## Decisions

- **The specs are the contract; `obc-formats` is only their code copy.** Producers (the host bakers) and consumers (the device readers) share one set of constants, so they cannot disagree about a byte. The crate holds no reader, because a reader carries policy: what to cache, what to skip, how to report errors. The rejected alternative was one reader crate per format that owns its constants; then a host producer would have to take a device reader with it.
- **`ByteSource` reads a byte range on request.** A reader keeps only small tables in RAM and streams the rest. A map can be larger than the device's RAM. The rejected alternative was to load a whole file or to map it into memory, which the device cannot do.
- **The quadtree block cache lives in `obc-formats`, next to `ByteSource`.** The map reader (`obc-reader`), the terrain reader (`obc-elevation`) and the route reader (`obc-route`) all walk quadtrees, and one cache with one eviction rule serves all three. The caller decides the slot count and whether a block enters the cache. (inferred: it is here and not in `obc-reader` because the terrain and route readers must not depend on the map reader.)
- **CRC-32 is its own crate with no dependencies.** The bootloader is small (32 KB) and links `obc-dfu`, which uses only `core`. A shared CRC must cost the bootloader nothing. The `slice-by-8` feature trades 7 KiB of flash for speed. The application firmware turns it on; the bootloader does not.
- **The `obc-ports` traits are synchronous and own no driver.** A sensor poll takes the newest value from a mailbox that the board fills. It does not start a bus transfer. So the app core runs the same way on the device, in the simulators and in tests, and needs no executor.
- **The `MapScene` walk methods are generic, not trait objects.** The renderer calls them once for each feature in a frame. A generic call is compiled for each concrete source, so the hot loop pays no dynamic dispatch.
- **`obc-file-source` is a separate host crate.** A host tool that needs to read a file takes this crate without taking a reader, a packer or a renderer with it.

## Words it introduces

- **Port**: a trait in `obc-ports` that a host implements, for example `SettingsStore` or `Sounder`.
- **Source**: in this system, a thing that gives bytes (`ByteSource`) or map features (`MapScene`). The word does not mean a source data set such as OpenStreetMap.
- **Microdegrees**: the integer unit of latitude and longitude in every crate here: degrees × 1,000,000.
