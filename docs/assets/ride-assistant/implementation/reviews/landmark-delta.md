# Landmark lane delta review

Scope: changed interfaces in RA02, RA08, RA09 and RA10 only, against L1–L3 from the first review. No builds, tests, repository edits or new full review.

- **L1: main finding resolved.** RA09 now names the shared mutable pixel-target phase into the existing resident frame, preservation across steps, reconstruction for full redraw/Sources/fresh captures, identity, cancellation and partial/error pixels. RA10 consumes it explicitly. A narrow remaining wording ambiguity was sent to the owner: partial DEFLATE steps need persistent budgeted history/state, whereas the current sentence releases all decoder scratch before every asynchronous presentation. Clarify that source/frame borrows are released, while explicitly budgeted host-owned decoder state may survive without a borrowed arena guard across await, or explicitly cancel/restart. Do not accidentally require lost history or unbudgeted copied buffers.
- **L2: resolved.** Required metadata has a separate faithful-representation gate, explicit streamed bounds, original/imported notices, unsupported-credit omission, and no generic font/transliteration expansion. RA10 preserves readable Sources after decode failure.
- **L3: resolved.** Normalized schedules cross the host boundary, landmark-only entities enter the shared pool, input hours indexes are remapped, and collision/non-service tests are required.
- **Related approach interface: ready.** RA02 owns the bounded source-linked approach producer; RA09 depends on RA02+RA08 and uses the same encoded coordinate/source/profile-mask or Unavailable. A real successfully routed landmark is required, preventing an information-only shortcut from satisfying the feature.

Ready after the one L1 sentence is clarified; no new architectural blockers found. Final exact-file hashes will follow that clarification.

## Final delta result

**READY. L1, L2 and L3 are resolved. No unresolved findings.**

Verified the final RA09 correction: decoder state/history is explicitly budgeted host-owned state between steps; only source/frame borrows end before asynchronous present; no arena guard crosses await; ownership ends on completion/cancel. This closes the remaining wording ambiguity without a new buffer/service architecture.

Reviewed exact SHA-256 file identities:

| File | SHA-256 |
| --- | --- |
| `02-place-queries.md` | `c78a6b36d80b9573150ffdaf4f7d97a89f69a71c681105d333097c30ee856c6a` |
| `08-landmark-sources.md` | `078d98d333baca94c84390758b4eda3f831f30472dfbefb0d1829f3bea8bda50` |
| `09-landmark-map.md` | `d271e1bd6c00147fe7cd796d18d7ee51483c2bfeee31ad0792c223636bbbd881` |
| `10-landmark-ui.md` | `d5e48136b5df930bbdd40b8e936f7c6161483e98e9fa765527f403512617f46e` |

Paths are under `/Users/timo/Documents/OSM-ride-assistant/docs/assets/ride-assistant/implementation/`. This is readiness of the plan, not implementation or hardware acceptance.
