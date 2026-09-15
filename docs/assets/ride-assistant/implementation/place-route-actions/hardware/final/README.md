# Final physical replay

Production source: `cc5bf0ebaf38ee9df07fe07982e4ac14168a4814`. The [identity record](identity.json) pins the exact ELF and offline inputs. [The flash log](flash-clear-final.log) records a successful verified write. The [device trace](device-clear-final-rtt.log) begins with firmware `0.1.0+cc5bf0e`.

The replay used the nRF54LM20 with the Swiss map and fixed GPS input at 46.723126, 8.194551, altitude 602 m and course 225 degrees. All actions used normal button input through the debug UART. The map, planner, storage and display paths ran on the device. GPS input is a stationary test feed, not a field recording.

| Operation | Seconds | Evidence |
| --- | ---: | --- |
| Find Resupply on the original Meiringen route | 7.105 | [Timing](clear-final-resupply.json) |
| Switch the Migros preview to Route here | 1.309 | [Timing](clear-final-destination.json) |
| Find Water after acceptance and recording discard | 2.333 | [Timing](clear-final-water-before.json) |
| Find Water again after opening a preview and pressing Back | 2.332 | [Timing](clear-final-water-after.json) |

These are individual input-to-present measurements at this location. They are not worst-case guarantees.

The direct route is 1045 m. Select accepts it. The test then stops and discards only its own recording, object 344. Removal commits at 67.351456 s. The next water query starts at 76.710003 s and produces four direct routes of 17, 255, 803 and 482 m. It does not use the visit planner or the prior 1045 m goal. This confirms that the concurrent recording update no longer loses the navigation stop.

The test opens a water preview, presses Back through Find and Assistant to Map, and runs the same query again. Both searches return four results. The [final map capture](device-clear-final-map.png) shows the rider marker with no route left on the map. No recording is active. The board is left with the same Swiss map and fixed GPS feed.

The prior [mode-switch and saved-Routes captures](../README.md) cover the unchanged UI. Independent adversarial review covered the final source and both physical records. No findings remain in these changes. Road riding, power-loss testing and broader hardware acceptance remain pending in [#1748](https://github.com/timohueser/OpenBikeComputer/issues/1748).

A separate recording-start warning observed during setup is tracked in [#1810](https://github.com/timohueser/OpenBikeComputer/issues/1810). Its sample completeness requires a separate check; this replay does not close that issue.

Stable device names, BLE addresses and serial numbers are removed from these published log copies. The local originals retain the full trace. Source hashes and timings are unchanged.
