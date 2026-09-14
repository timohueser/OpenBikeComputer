# RA09 — Store and read compressed landmark content in ordinary maps

Parent: #1734. Depends on RA02 and RA08. Keep content in the installed map object and existing transfer path.
No separate SD asset filesystem, image object catalog or phone-only lookup service.

Implementation dependencies: [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737), [RA08 — #1743](https://github.com/timohueser/OpenBikeComputer/issues/1743).

## Format and pipeline

Define one optional OBCM landmark section with a bounded spatial index and fixed records that
reference variable text, provenance and independently compressed photo blobs. Store QID/category,
display coordinate, optional mapped approach/hours link, actual article language and bounded content
references. Resolve explicit OSM joins through RA02's packer approach contract and encode its
bounded coordinate/source/profile-mask result or Unavailable; a landmark with no service record
still uses this producer. Require one real landmark with a mapped, successfully routed approach.
Separate cheap discovery metadata from selected-item text/photo reads. Do not overload fixed service
POI records or small name fields with prose. Use checked lengths/offsets and one exact format
version; no compatibility layer. Spell out limits and corruption semantics in OBCM_Spec.

Implement across `obc-formats`, `obc-reader` (`ByteSource`/`WindowSource`), `obc-pack` serialization
and cell cutting, `obc-bake` cell/cache orchestration, `obcm-assemble`, and
`obc-web-assemble`/builder assembly. A whole-map-only feature is incomplete. Define geographic
ownership at cell boundaries, QID deduplication and deterministic precedence. Remap pooled offsets
and shared content on assembly. Use the existing schedule encoding/pool for landmark hours even when
the source entity has no service record. Intern and remap schedules by content/source policy, never
retain input hours_ref indexes. Test a non-service castle and two cells with colliding input hours
indexes. Validate optional absence, shuffled input cells, duplicates, map boundaries and corrupt
references. Update shared vectors, format consumers, builder cache keys and immutable fixture
revisions together. Use current map install/catalog validation and ordinary host/board map readers.

## Photo format and bounded execution

Choose one lossless independent stream per 216 × 240 RGB222 photo. Start with host DEFLATE and a
small-window no_std decoder feasibility measurement; lock a supported maximum history window and
output count in the contract before implementing the reader. If that cannot meet existing device
scratch/stack limits, choose one measured simpler bounded codec before merging the format. Do not
ship a speculative codec registry or multiple fallbacks. Six-bit packing alone is about 38.9 kB; the
5–10 kB result on seven photos is a target, not a universal size guarantee.

Decode via bounded map reads and a scanline/tile-sized output buffer. Do not allocate another
51,840-byte permanent image or decompress all map content to display one picture. Map/selection
changes cancel/invalidate work. Keep rendering responsive and return scratch after completion or
failure. Reuse the app preparation/scheduler and existing palette conversion; draw consumes ready
bounded data.

Extend the existing render/preparation path with an explicit bounded mutable pixel-target phase. The
host owns the same resident Frame64 used for ordinary rendering; the decoder writes completed rows
there outside screen draw. App retains only map/QID/photo identity, rectangle, render generation and
completion state. No second framebuffer or full photo allocation. A fresh base clears the photo
rectangle, draws the surrounding UI, then preserves completed rows between decode steps. Partial
rows may appear with a loading state; they must carry the selected source identity. A fresh
framebuffer, base redraw or dismissed Sources drawer invalidates the rectangle and replays decode
from the compressed source. Suspend/cancel image writes while a drawer covers or dims the base;
decoding must not overwrite it. Selection/error clears stale pixels and restarts or shows
text-only/error state. Release source/frame borrows before asynchronous presentation. Retain only
the explicitly budgeted host-owned decoder state/history between steps, borrowing it for each
work step; never hold an arena guard across await. Release decoder ownership on completion or
cancellation. Obey the existing frame ownership/acknowledgement protocol.

Use this same contract on board, interactive simulator and fresh headless frames. Headless capture
steps normal work until complete or a bounded error, instead of bypassing it with a decoded asset.
Prove open/close Sources, full redraw, partial failure and cancellation reconstruct the correct
photo without hidden file reads in screen draw. Keep this a rendering phase, not a new service.
Decoder lifetime and any arena use must have an explicit simultaneous-use census with map rendering,
terrain, navigation and storage operations.

Reject oversized output, invalid palette indices, invalid/truncated compressed data, out-of-section
references and integer overflow. A bad photo leaves text, Sources, Back and available Visit usable;
clear stale pixels so another landmark's image is never attributed to this one. SD errors must not
be converted to a real source's missing-photo fact.

## Acceptance and verification

Byte-exact RGB222 roundtrip, independently seekable photos and bounded memory are mandatory. Test
adjacent-cell merge, QID/content deduplication, all-empty/absent section, source hash invalidation,
malformed offset/count/compressed stream, I/O failure and selection/map cancellation. Build a real
regional map through bake → cut → assemble, then query and decode it through the ordinary flat-card
reader in the simulator. Report actual compressed distributions and metadata/index overhead. Run
whole affected format/reader/pack/assemble/decoder suites, WASM build/contract checks and scoped
Clippy/no_std. RA13 coordinates the sole final shipping resource build. Physical SD latency and
stack high-water remain device acceptance; desktop timing is not a substitute.
