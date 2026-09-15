# Physical place-action replay on d55493fed

This evidence covers firmware source `d55493fedefc92ad857f16a318f6921f4922b02f` on the nRF54LM20 device with the real offline Meiringen map. The ELF SHA-256 is `eb426ae9f50642f9d9c8b11b8eec8a864d2ef7a4b794f009216ecc4dbfac9f05`. [identity.json](identity.json) records the source, binary hash and hashes of the retained evidence. The boot line in [device-mode-fix-rtt.log](device-mode-fix-rtt.log) identifies `0.1.0+d55493f`.

The later checkpoint-clear fixes `38c66527e` and `cc5bf0eba` have a separate [final physical replay](final/README.md). The bench checks passed. Road riding and power-loss acceptance remain pending.

## Flash and measurements

[flash-mode-fix.log](flash-mode-fix.log) records the verified flash command and successful completion:

```sh
probe-rs download --chip nRF54LM20A --non-interactive --verify --disable-double-buffering /Users/timo/Documents/OSM-agents/ride-assistant-speed/.artifacts/place-actions/obc-fw-place-actions-mode-fix
```

Each timing JSON retains the exact input and completed frame log lines. Times include the work from that input through the recorded frame; they are one replay at this location, not worst-case guarantees.

| Operation | Seconds | Exact evidence |
| --- | ---: | --- |
| Active-route Resupply search | 6.895 | [final-resupply.json](final-resupply.json) |
| Switch to Route here | 1.263 | [final-destination-first.json](final-destination-first.json) |
| Switch back to Add detour | 1.833 | [final-detour.json](final-detour.json) |
| Switch to Route here again | 1.471 | [final-destination.json](final-destination.json) |
| Water search after acceptance | 4.588 | [post-accept-water.json](post-accept-water.json) |
| Water search without recording, before preview and Back | 2.259 | [no-recording-water-before.json](no-recording-water-before.json) |
| Same search after preview and Back | 2.267 | [no-recording-water-after.json](no-recording-water-after.json) |

The active-route and mode transitions are in [device-mode-fix-rtt.log](device-mode-fix-rtt.log). The search after acceptance is in [device-after-preview-rtt.log](device-after-preview-rtt.log). The independent session without recording is in [device-free-preview-rtt.log](device-free-preview-rtt.log).

## Observed results

[device-route-here.png](device-route-here.png) shows Migros as a direct destination, 1 km and 4 m ascent, with both mode arrows. [device-routes-after-accept.png](device-routes-after-accept.png) shows only the original Meiringen route in saved Routes after acceptance. Internal route objects remain available to navigation and recovery.

The session without recording starts after the test ride object 331 is discarded. Its Hold input occurs at 799.833500 s; removal commits at 800.392481 s. The operator then opens the map from Home and starts the first water search at 811.815690 s. A preview opens from Find at 828.988104 s. Back from VisitReview at 830.105122 s, followed by Back through Find and Assistant, returns to Map. The next water search starts at 833.849986 s and finishes normally.

[device-free-map-after-cancel.png](device-free-map-after-cancel.png) shows the rider marker and base map with no preview route left on screen. This check follows a completed discard; it does not validate the later fix for a checkpoint-clear race during recording writes.

The map and route input references are in the [simulator input record](../simulator/inputs.json). This package contains the recorded logs, timing results and resident-framebuffer captures only. It does not include firmware binaries, card images or duplicate per-phase logs. No test, build, device replay or UI sweep was run to package it.

Stable device names, BLE addresses and serial numbers are removed from these published log copies. The local originals retain the full trace. Source hashes and timings are unchanged.
