# Requirements draft — decisions and source notes

This file accompanies [requirements-draft.md](requirements-draft.md) and [acceptance-targets.md](acceptance-targets.md). Keep decisions, rationale, and source observations separate from the requirement text entered into the console.

The revised draft contains 357 active requirements in 27 groups. IDs run through REQ-358; REQ-321 is retired after tunnel warnings were removed from scope, and its ID is not reused. The owner's feedback resolves scope decisions, but it is not verification evidence. Some requirements describe current behavior and others describe required future work. No entry is assigned a passing status.

## Scope

Include the physical device, firmware, companion, map builder, desktop application, public website, and map and firmware delivery. Include the current Assistant features, Peak View articles/photos, and the additional features required below. Exclude weather. Keep phone Recently Deleted separate from device storage: phone trash lasts 30 days; device rides are deleted only by an explicit rider action, including confirmed factory reset.

The nRF54LM20 is a design constraint. Development-kit peripherals are not automatically final product requirements. Release-console, CI, fixture, simulator, and developer-tool requirements belong in their own specifications.

## Decisions and deferred acceptance details

The scope review is complete for this working draft. **Accepted** records owner decisions. **Open** and **TODO** retain later acceptance or design details; they do not request another questionnaire before publication. **Proposed** identifies a review suggestion that has not become an approved value. The draft must not treat unresolved acceptance criteria as passing release gates.

### D01 — Position when no ride is recording

**Accepted:** Position-dependent features request GNSS independently of recording. Acquisition shows its waiting state and does not itself start recording. Return to idle GNSS behavior when no function needs it. See REQ-222 and REQ-315.

### D02 — Navigation scope

**Accepted:** Sound an auditory cue when the rider leaves the planned route. Do not add automatic rerouting or turn-by-turn maneuver instructions. Recovering from an accidental departure is an explicitly requested Back on route mode in the Ride Assistant. See REQ-292–296.

**Open:** Rejoin-point selection and off-route trigger/repeat conditions are TODO. A suggested rejoin must show any skipped route section before acceptance. Sharp-turn warnings are separately required under D22; they are not turn-by-turn guidance. Tunnel warnings are deferred.

### D03 — Map installation during a ride

**Accepted:** Block map upload while a recording is active, paused, or awaiting save/discard. Show the reason before transfer. See REQ-282.

### D04 — Map display settings

**Accepted:** Retain all three independent switches: contours, map clock, and map scale bar. The current provisional contour comment does not supersede this decision. See REQ-283.

### D05 — Release languages

**Accepted:** English, German, French, and Spanish across the device, companion, desktop, and public website/map builder at release. The current English-only companion is an implementation gap. See REQ-211. This commitment concerns the user interface; articles retain their explicit source-language fallback.

### D06 — Unknown direction and altitude

**Accepted:** Labelled north-up map when heading is unavailable; manual Peak View browsing remains usable. Clearly distinguish unreferenced barometric altitude from referenced elevation. See REQ-280 and REQ-302.

**Open:** Numerical freshness and accuracy limits are TODO. Compass calibration interaction depends on final hardware validation and is not decided by accepting the display fallback.

### D07 — Geography and essential map features

**Accepted minimum:** Europe, United States, Canada, Australia, and New Zealand. The essential map baseline is accepted: road classes, cycleways and trails, water, settlements and names, forests/open land, close-scale buildings, bridges/tunnels, contours, and service places. These are geographic objects to display, not a second list of application functions. See REQ-033, REQ-284, and MAP-01–05.

**Open:** Exact country/territory boundaries, including islands and dependencies, belong in a release coverage manifest. The original North America wording still needs a decision on Mexico, Central America, the Caribbean, and Greenland. Europe is not limited to the EU. Feature scale thresholds and source availability conditions remain TODO.

Nearby landmark search radius also remains open; current code uses 10 km. Do not infer complete opening hours, terrain, articles, or photos from the geography requirement.

### D08 — Map management scope

**Accepted:** One active map replaced through USB. No multiple-map selector or phone map transfer. No automatic map updates are added.

### D09 — Route import and limits

**Accepted:** Bridge disconnected segments with straight lines only when their order is clear; otherwise warn and refuse import. For multiple tracks/courses, warn and let the rider select a single track/course or cancel. Reduce excess track points on the application before transfer and show an information popup. Warn and drop the last excess waypoints. Route reversal belongs in the applications; route editing and device reversal are outside scope. See REQ-287–291.

**Proposed interaction detail:** Show inserted gaps in the preview as straight connections, without calling them verified riding connections. Apply waypoint truncation after reversal, using the final route direction. Preserve route endpoints and waypoint associations during point reduction. These details make the accepted policies visible and consistent.

**Open:** Input/device limits, maximum simplification deviation, and examples of clear versus ambiguous ordering. The existing 32-waypoint reader window is not evidence of a 32-waypoint whole-file import limit; do not copy it as one.

### D10 — Navigation and recording recovery

**Accepted:** After restart, offer Resume navigation for both ordinary routes and Assistant journeys when the saved sources and a fresh position are valid and unambiguous. Restore the applicable progress and visit/return phase. Let the rider decide independently of Continue/Discard recording. Resuming navigation does not start recording; recording recovery does not silently accept or delete the navigation plan. Declining resume leaves guidance inactive and preserves route files. Trips never advance automatically. See REQ-055 and REQ-096–097, now grouped with Navigation and detours.

**Implementation issue:** [#1860 — Offer navigation resume after restart for ordinary routes and Assistant journeys](https://github.com/timohueser/OpenBikeComputer/issues/1860).

Assistant recovery restores a navigation journey such as “visit this café, then return to the planned route.” Recording recovery restores the ride log. Current explicit Assistant recovery is the basis to extend; ordinary route files already persist, but their guidance is not automatically restored.

**Deferred:** A navigation-only choice when selecting a new route is not added. Current route-start behavior normally starts recording; the restart decision does not change that separate interaction.

### D11 — Export timestamps

**Accepted:** Preserve valid recorded sample timestamps in exported GPX. See REQ-162. The inspected Rust and Swift exporters omit point timestamps, so this is required work.

### D12 — Recording loss and capacity

**Accepted:** No accepted data loss on a successful orderly save/shutdown; no more than **30 seconds** lost after sudden power removal at 1 Hz with supported healthy storage; a 72-hour continuous recording with external power as needed; capacity for thirty 12-hour rides at 1 Hz without phone offload. See REC-01–04 and REQ-313–314.

**Basis:** Samples are 20 bytes. The two sample payloads are about 4.94 MiB and 24.72 MiB respectively, before overhead and reserves. The visible ride catalog currently holds 32 entries. Storage volume is modest, but duration, counter, UI, and physical-storage verification are still required. Battery runtime is a separate TODO.

The recorder requests a checkpoint every ten seconds and stages up to 16 samples. These constants do not prove the 30-second recovery-loss bound. Test power removal on real supported storage. Other GNSS cadences still need a bound. Damaged storage is a separate fault case.

**Deferred:** A direct Save recovered ride action is not required in this draft. The current Continue then Finish path remains the baseline.

### D13 — Personal pace and accuracy

**Accepted recommendation:** Personal pace learning is mandatory for all predicted duration and arrival values. Use 300 km of prior usable history and require mean absolute percentage error below 10% for moving-time predictions on unseen rides. Evaluate each rider, with separate reports for supported riding categories and long rides. Arrival assumes no future breaks and must say so. The current fixed bike-profile estimator is not an accepted fallback. See REQ-180–181 and REQ-299–301.

This records “sounds good” as acceptance of the proposed benchmark: average percentage error, 300 km, and moving time. It does not assert that every individual estimate falls within 10%. Report the error distribution as well as its mean and freeze the scoring protocol before evaluation.

**Open:** Exact prediction checkpoints/horizons, usable-history criteria, rider sample, category coverage, confidence, and eligibility rules remain TODO. No estimate appears during insufficient learning or outside validated conditions. Training must not use the future ride data against which a prediction is scored.

Distance and ascent accuracy use measured courses/reference traces with stated uncertainty and a fixed ascent definition. Separate field measurement from calculation tests. Tolerances and conditions remain TODO; another bike computer is a comparison device, not absolute ground truth.

### D14 — Sensor scope and power-meter zeroing

**Accepted:** Bluetooth Low Energy heart rate, power, and cadence only. No ANT+ or wheel-speed sensor requirement. See REQ-183–190.

**Answer:** The usual on-bike “calibration” action is a zero-offset operation, similar to taring a scale. The rider unloads and positions the pedals/cranks as the manufacturer instructs; the head unit sends a command and reports the meter's success or failure. The meter performs the adjustment. It is not the bike computer recalculating the meter's factory calibration.

Other computers offer it: [Wahoo ELEMNT/BOLT/ROAM instructions](https://support.wahoofitness.com/hc/en-us/articles/115000637870-Using-a-Power-meter-with-ELEMNT) describe sending the calibration command and reporting completion. That support page does not prove every meter supports calibration over Bluetooth.

It is not universally necessary to expose this control on OpenBikeComputer to receive power values. Correct zeroing/setup can still matter for accurate measurements, depending on the meter. Some meters have automatic zeroing and a manufacturer-app procedure; [Wahoo's SPEEDPLAY instructions](https://support.wahoofitness.com/hc/en-us/articles/4441997272082-Understanding-Automatic-and-Manual-Calibration-for-SPEEDPLAY-POWER-pedals) describe both. Crank-length configuration for pedal meters is a separate setup concern and may require the manufacturer's app.

**Accepted:** Require a device Zero power meter action when the meter exposes it over a supported Bluetooth Low Energy interface. Detect unsupported operation; explain preparation; require an explicit rider request; show progress, confirmed success, meter-reported failure, disconnect, and timeout. See REQ-330–331. A tested model/interface list and timeout values remain TODO; proprietary operations are not presumed supported.

### D15 — Factory reset

**Accepted:** Reset all device settings and delete all files except the installed map. Remove phone pairing, saved sensor identities, routes, trips, rides, recovery state, and learned pace state. Require clear confirmation. See REQ-214 and REQ-305–306.

Phone archives and remote service copies are outside a device factory reset. Firmware remains installed. Factory reset must clear rider state even if some is in internal storage rather than map-card files. The current settings-only reset does not meet this requirement.

### D16 — Usability and characters

**Accepted:** Color-accessible default presentation or a persistent accessibility mode; readability in full sunlight and complete darkness. Glove, wet-hand, angle, and viewing-distance specifications remain deferred. Text normalization and Romanian Ș/ș/Ț/ț support are required. See REQ-253–254, REQ-304, REQ-307, and REQ-332–333.

Normalize display text to precomposed Unicode where available and replace unsupported typographic punctuation with readable equivalents. Preserve supported letters and original names in application libraries and source exports. Existing article preparation already normalizes some punctuation, but that does not establish normalization across all content paths.

**Greek/Cyrillic assessment:** Native support is a bounded extension of the current bitmap renderer. [Terminus](https://terminus-font.sourceforge.net/) already provides Greek and Cyrillic repertoire. Extend the conversion character set and glyph mapping, verify actual source glyphs in each selected size, and check the prepared data and text-coverage filters. Modern precomposed names do not require replacing the renderer with a complex text-layout system. This does not promise all historical or extended script variants.

Calculated bitmap-storage costs with the present three 1-bit text sizes (12×24, 14×28, 16×32), padded to rows of 16 glyphs:

| Addition | Extra bitmap bytes | Approximate size |
| --- | --- | --- |
| Romanian Ș/ș/Ț/ț alone | 2,384 | 2.33 KiB |
| Entire basic Greek/Coptic U+0370–03FF and Cyrillic U+0400–04FF slots | 59,600 | 58.20 KiB |
| Both blocks plus Romanian | 61,984 | 60.53 KiB |

These are layout calculations, not measured firmware deltas. A selected modern-letter subset can be smaller. Font mapping/code overhead is additional. The large clock font stays unchanged. The bitmaps are static; this extension does not require a larger framebuffer or per-character heap allocation. Confirm final linked storage and RAM when implemented.

**Important source finding:** `host/obc-pack/src/poi.rs::normalize_name` folds service-place names to 24-byte ASCII and replaces unmappable letters with breaks. Fonts alone therefore cannot restore Greek/Cyrillic names removed during baking. The packed-name contract and producer/reader paths need changes for native names; UTF-8 byte limits must preserve whole characters. `host/obc-pack/src/landmarks/text.rs::supported` also hard-codes the current repertoire and needs to agree with the chosen fonts.

**Accepted final policy:** Add native modern Greek/Cyrillic support without widening fixed map-name fields or records, raising existing text byte limits, or storing duplicate native/transliterated names solely for this feature. Use a readable transliteration or shortened name within the existing byte limit when necessary. Keep normalization and Romanian support. The owner accepts the extra font bitmap storage and may revisit the feature later. See REQ-307 and REQ-332–335.

**Map-size finding:** `specs/OBCM_Spec.md` section 7.3 and `POI_NAME_LEN` define a 24-byte name field within a fixed 64-byte POI record. Service names currently use ASCII, while summit names already use UTF-8 in the same space. Allowing service UTF-8 changes permitted contents and validation, not the field width. A 24-byte field can hold 24 ASCII letters or 12 typical two-byte Greek/Cyrillic letters; mixed names depend on their encoded bytes. Longer names need the bounded fallback. This is a real character-capacity tradeoff, not a requirement for larger records.

Landmark names and article text already use variable-length UTF-8 with byte caps. Keep those caps. Their actual payload length or compressed map size can change when different content is retained; no byte-for-byte size guarantee is claimed. Additional scripts must not require wider storage throughout the map or a second stored spelling. The existing ASCII fold is not already a complete Greek/Cyrillic transliterator.

**Later acceptance detail:** Select the exact modern repertoire and representative native/fallback names. Verify both font coverage and the complete producer-to-display path, and compare fixed record widths and declared byte caps.

### D17 — Power targets

**Accepted:** Add explicit requirements and TODO acceptance values for runtime profiles, standby/off drain, charging time, regulated dynamo behavior, electrical limits, low-energy warnings/shutdown, battery aging, and update reserve. See REQ-308 and PWR-01–14.

Current GNSS interval settings permit intervals of ten seconds or more while the recorder rejects movement intervals at or above ten seconds. REQ-220 deliberately requires supported selected cadences to work. Development-board operation does not establish final battery runtime.

### D18 — Updates and owner-built firmware

**Accepted:** Normal field updates require device confirmation and a verified previous image for rollback. Failure to reserve it blocks installation. Provide a separate documented owner/development flashing path. See REQ-244–249 and REQ-309–310.

**Deferred:** Background update checks and notifications are not added. Manual update checks remain required.

Current board installation is disabled; uploading a package does not demonstrate these required behaviors.

### D19 — Hardware targets

**Accepted:** Include requirements and explicitly TODO values for water/dust exposure, charging exposure, temperatures, humidity, drops, vibration/shock, mounting, size/mass, service life, and replaceable parts/sealing. See REQ-311 and PHY-01–19. No protection rating or certification is claimed.

### D20 — Platforms and waits

**Accepted:** Recent macOS, Windows, Linux; Firefox, Chrome, Safari, Edge, with device operations limited to browsers that expose the necessary APIs. Exact versions/distributions/architectures are TODO. Existing iOS companion support stays in scope.

**Answer:** Yes, the wait targets cover device actions. The table also covers application import, map preparation, transfers, and updates so waits across the complete interaction are specified. All values are TODO; see PERF-01–22 and PLAT-01–07.

### D21 — Emergency location and route to start

**Accepted:** A quickly accessible emergency location screen. Route to start means an offline route from the current position to the **first point of the selected route**, using the selected bike profile. Preview and explicit acceptance remain required. See REQ-297–298 and REQ-316–319. Returning to the recording origin or a parked car is outside scope.

**Proposed emergency format:** WGS 84 decimal latitude/longitude with labels, coordinate format, fix status, and age. Final precision and access action count remain TODO. This is location display, not a remote SOS service.

**Open:** Whether reaching the route start offers an explicit Start selected route action or transitions guidance automatically. Either behavior must preserve the recording.

### D22 — Route-based sharp-turn warnings

**Accepted final scope:** Warn only about qualifying sharp turns on the active route, with both sound and a visual indication. Derive the warning from route geometry and known progress. No free-ride sharp-turn warnings. Tunnel warnings and general mapped-road hazard warnings are excluded from these requirements; any future addition needs a new explicit scope decision. REQ-320 and REQ-322–323 are updated; REQ-321 and criterion NAV-06 are retired without reusing their IDs.

**Code assessment:** `firmware/obc-route/src/matcher.rs` matches position against the selected route and maintains along-route progress. `firmware/obc-route/src/nav.rs` snaps planning endpoints to the routing graph. Neither establishes continuous road identification for arbitrary riding, especially at parallel roads, junctions, or grade-separated crossings. No existing tunnel-warning metadata/consumer path was found in the inspected baker, map formats, and route code.

A planned-route tunnel warning would not necessarily require a complete live road matcher: map preparation or route import could associate the planned route with tunnel data ahead of time. That still needs a new route-to-map association and hazard-data path, which an arbitrary imported GPS track does not supply. Deferring that work fits the current scope.

**Open numerical targets:** Sharp-turn geometry, speed and valid-position conditions, warning lead time/distance, repetition, priorities, and settings remain TODO. Include noisy geometry, loops, and imported straight-line gaps in acceptance cases; those must not be assumed to prove a real road bend.

### D23 — External services and trail blazes

**Accepted:** External-service export/sync is required where technically possible, with Strava and intervals.icu as initial named targets and broad provider coverage as a goal. Toggleable trail blazes are included, subject to later review. See REQ-285–286 and REQ-324–329.

**Open:** Prioritize further services; decide which application owns integrations and whether automatic upload is part of the first supported path. Proposed starting point: companion with explicit per-service opt-in and truthful upload state. Provider approval/access, file fallback, supported fields, and historical upload scope are TODO in the support matrix. A generic GPX export must not be advertised as a completed direct integration.

For blazes, supported symbols and overlapping marked routes remain TODO; no broader hiking mode is implied.

## Review additions (16 September)

A review of the draft against the product envelope added entries REQ-336–358 and regrouped the draft into 27 groups. None of these is an owner decision yet.

**Group splits.** Maps and map management became Map builder and map service, Map installation, and Map display on the device. Controls, settings, language, and time became Device controls and interaction, Languages and text rendering, and Settings, preferences, and factory reset. Supported applications and operating limits became Supported platforms and services and Capacity, performance, and robustness. Clock sources and local time (REQ-216–217) moved to Position, heading, and time. Sharp-turn and warning entries (REQ-320, 322, 323) moved to Navigation and detours; Emergency location stands alone. Each group follows one surface boundary so a group can carry one minimum evidence level in the requirements console.

**Requirement splits.** Where one entry held clauses that need different tests, the second clause received a new ID: REQ-336 (reset deletion scope, from 214), REQ-337 (interrupted reset, from 306), REQ-338 (resume decision, from 096), REQ-339 (zero versus missing cadence, from 187), and REQ-340 (recovery continuation, from 146). 175 entries still contain two or more shall clauses; split further during owner review only where the clauses need different tests.

**Proposed additions.** REQ-341–343 (card removal, storage status, diagnostics), REQ-344–345 (idle shutdown, charging while off), REQ-346–347 (sound control, button lock), REQ-348–349 (compass calibration, map orientation), REQ-350–351 (trip stage status, ride identity), REQ-352 (background transfer), REQ-353–354 (map compatibility after update, bootloader recovery), REQ-355–356 (time and date format, rider documentation), REQ-357 (card readability), and REQ-358 (emergency information). Entries marked PROPOSED (347, 349, 357, 358) need an explicit scope decision. New acceptance rows: PWR-15 and CLIMB-01.

**Noted, not changed.** REQ-036 accepts TCX on the phone while REQ-037 accepts only GPX on the computer. REQ-187 keeps a numeric limit in its text while other limits live in the acceptance tables. REQ-294 is a negative scope statement, REQ-252 a design constraint, and REQ-089 starts a recording on visit acceptance next to REQ-088 and REQ-135; all three are decisions rather than errors.

## Deferred features

The following remain outside the requirements:

- Weather forecasts, rain maps, and weather alerts.
- Last food/water/repair stop before a gap and opening-at-arrival filtering.
- Named climbs and passes beyond current climb facts.
- Generic route-join target picker and return to a parked bike/car or the recording origin.
- Terrain sunset/shade, computed viewpoints, and panoramas from another observer position.
- Tunnel, obstacle, and other mapped-road hazard warnings; sharp-turn warnings without an active route.
- One-action rider waypoint creation/export.
- Predicted arrival ranges beyond the required validated personal estimate.
- User-configurable charging current and battery time-remaining estimates.
- Additional sensors, ANT+, training metrics, and a separate hiking mode.
- Automatic desktop updates and particular installer/signing schemes pending distribution decisions.

Back on route is now required future work. Road blocked and Worth a detour remain unselected Assistant placeholders; the existing map Detour feature stays included. Landmark and peak articles are already included; the old “Wikipedia sights” backlog wording is not a separate feature.

## Source map


Product code was inspected at local commit `5ae466a96c6f32e68a2dd78670e9980bd0a4cbb0`. Remote `develop` was 14 commits ahead at the comparison used for this draft. Its changed paths concerned verification tooling, tests, policies, and firmware-release documentation; no new device feature was inferred from them. The current remote firmware-update page was also read. GitHub issues were read on 16 September 2026.

Links below identify the basis for a group, not proof that every proposed requirement is implemented. Where prose or an old issue conflicts with current code or a newer normative contract, the draft records that conflict instead of preserving the older behavior.

| Requirements | Principal source paths in this repository | Product decisions and limits |
| --- | --- | --- |
| 001–035 | `builder/app/src/components/coverage/`; `builder/app/src/components/device/MapSend.svelte`; `firmware/obc-app/src/screen/map.rs`; `firmware/obc-app/src/map_catalog.rs` | [One-map decision #1420](https://github.com/timohueser/OpenBikeComputer/issues/1420). Per-kind upload validation is still incomplete according to the companion-link documentation. |
| 036–057 | `companion-ios/Packages/OBCKit/Sources/OBCFormats/RouteImport.swift`; `OBCUI/Import/ImportFlowModel.swift`; `OBCUI/Trip/TripDetailView.swift`; `builder/app/src/components/device/RouteDrop.svelte`; `TripDropDialog.svelte`; device `screen/route_menu.rs`, `route_overview.rs`, `trip_delete.rs` | Phone and computer import support differ. Multi-file grouping does not establish multi-track import semantics. |
| 058–074 | `firmware/obc-app/src/navigator/following.rs`; `screen/nav_route.rs`; `screen/detour.rs`; `screen/route_swap.rs`; `builder/presets/schema.json` | [Navigation #1400](https://github.com/timohueser/OpenBikeComputer/issues/1400). Physical planning and recovery acceptance is distinct from source completion. |
| 075–113 | `docs/content/software/ride-assistant-study.md`; device `screen/find_place.rs`, `journey.rs`, `whats_next.rs`, `easier.rs` | [Assistant scope #1734](https://github.com/timohueser/OpenBikeComputer/issues/1734). Current docs include later cancellation, recording-start, and restart decisions. |
| 114–124 | `docs/content/software/ride-assistant-study.md`; `screen/landmarks.rs`; `screen/landmark_photo.rs`; `firmware/obc-app/src/settings.rs` | [Regional content #1805](https://github.com/timohueser/OpenBikeComputer/issues/1805), [multilingual content #1832](https://github.com/timohueser/OpenBikeComputer/issues/1832). Closed sites remain readable. |
| 125–134 | `firmware/obc-app/src/screen/peak_view.rs`; `screen/peak_article.rs`; `firmware/obc-fw-nrf54l/README.md` | [Peak articles #1830](https://github.com/timohueser/OpenBikeComputer/issues/1830), [merged implementation #1849](https://github.com/timohueser/OpenBikeComputer/pull/1849). The board README still contains an older Select-return description; the current article path takes precedence. |
| 135–151 | `firmware/obc-app/src/recorder.rs`; `screen/ride_control.rs`; `screen/ride_recovery.rs`; `screen/rides.rs`; `screen/ride_detail.rs` | [Ride lifecycle #1398](https://github.com/timohueser/OpenBikeComputer/issues/1398) has stale automatic-retention prose. Current archive contract takes precedence. |
| 152–168 | `specs/Ride_Archive_Contract.md`; `specs/Ride_Archive_Metadata.md`; `OBCUI/Main/RideSyncCoordinator.swift`; `OBCUI/Main/MainScreenModel.swift`; `OBCFormats/GPXRideEncoder.swift`; `firmware/obc-route/src/track.rs`; `apps/obc-desktop/src/rides.rs`; `builder/app/src/components/device/RideExport.svelte` | Exact archive proof is separate from transport success. Timestamp export is now required; current exporters still need it. Some desktop archive helpers exist without proving end-to-end receipt delivery. |
| 169–182 | `firmware/obc-app/src/stat_fields.rs`; `screen/statistics.rs`; `screen/climb.rs`; `altitude.rs`; `firmware/obc-route/src/eta.rs` | [Adaptive-time research #1780](https://github.com/timohueser/OpenBikeComputer/issues/1780) is not integrated product behavior. |
| 183–190 | `firmware/obc-app/src/screen/settings/sensors.rs`; `firmware/obc-app/src/settings.rs`; `recorder.rs`; `docs/content/software/companion-link.md` | Current quantity slots are heart rate, power, and cadence. Protocol service support alone is not a wheel-speed product commitment. |
| 191–202 | `docs/content/software/companion-link.md`; `companion-ios/CLAUDE.md`; `firmware/obc-app/src/screen/settings/bluetooth.rs`; `companion-ios/OBCProtocol.md` | [Forget-phone issue #481](https://github.com/timohueser/OpenBikeComputer/issues/481) remains open. A desired reconnection guarantee is not evidence of current reliability. |
| 203–218 | `firmware/obc-app/src/settings.rs`; `screen/quick_drawer.rs`; `screen/context_drawer.rs`; `screen/settings/display.rs`; `screen/settings/reset.rs`; `screen/settings/datetime.rs` | The owner requires all four languages across all released user interfaces and confirms all three map toggles. Existing companion localization and the provisional contour comment do not meet that intent. |
| 219–230 | `README.md`; `firmware/obc-app/src/screen/settings/power.rs`; `firmware/obc-fw-nrf54l/src/sensors.rs`; `firmware/obc-platform/src/backlight.rs`; `hardware/shared/power_management.kicad_sch` (inventory only) | The README's four-day goal is not a measured runtime. Charging behavior needs final-hardware decisions and evidence. |
| 231–239 | `firmware/obc-app/src/screen/route_cleanup.rs`; `catalog_state.rs`; `specs/Ride_Archive_Contract.md`; companion-link storage contract | Current cleanup is explicit. Old retention and map-set comments remain in places and were not adopted. |
| 240–251 | Current `docs/content/software/firmware-updates.md`; `firmware/obc-app/src/screen/dfu.rs`; update-facing companion file inventory | [Update intent #773](https://github.com/timohueser/OpenBikeComputer/issues/773), [installation work #1391](https://github.com/timohueser/OpenBikeComputer/issues/1391). Old service/container details in #773 are superseded; the draft avoids depending on them. |
| 252–261 | `AGENTS.md`; `README.md`; `docs/content/hardware/index.md`; `firmware/obc-fw-nrf54l/README.md`; hardware file inventory | Most final environmental and service targets are owner decisions, not established by repository files. Schematics were not electrically reviewed. |
| 262–266 | `README.md`; project license declarations; local archive and transfer architecture | Privacy guarantees are proposed release behavior. No privacy/security audit was performed. |
| 267–273 | `apps/obc-desktop/README.md`; `companion-ios/CLAUDE.md`; current platform and application inventory | [Desktop acceptance #994](https://github.com/timohueser/OpenBikeComputer/issues/994), [future backlog #1518](https://github.com/timohueser/OpenBikeComputer/issues/1518). A build target is not proof of supported distribution or hardware operation. |
| 274–281 | `firmware/obc-app/src/app.rs`; `firmware/obc-fw-nrf54l/src/sensors.rs`; `firmware/obc-fw-nrf54l/README.md`; `firmware/obc-app/src/recorder.rs` | The owner has confirmed idle acquisition and labelled north-up/manual-panorama fallback. Numerical freshness limits remain TODO. |

Swift paths beginning with `OBCUI/` or `OBCFormats/` are relative to `companion-ios/Packages/OBCKit/Sources/`. Device paths beginning with `screen/` are relative to `firmware/obc-app/src/`.

## Implementation gaps and draft limits

The draft states desired outcomes. It does not certify that current code meets them. In particular:

- Independent feature-driven GNSS acquisition, off-route sound, Back on route, emergency location, Route to start, hazard warnings, and trail blazes need implementation/verification review.
- Map validation and safe replacement require end-to-end storage evidence; length and CRC alone are insufficient.
- Import multi-track, gap, simplification, reversal, and waypoint policies now reflect owner intent; current parser behavior is not their specification.
- Exporters need valid per-point timestamps.
- The personal model needs independent evidence for the less-than-10% requirement before ETA can ship. Current generic estimation does not qualify.
- All four UI languages, color accessibility, text normalization/Romanian support, BLE power-meter zeroing, and the expanded factory reset need surface-specific work. Native Greek/Cyrillic support now has a fixed-budget requirement and bounded fallback.
- The GNSS-cadence/recorder-gap mismatch must not become intended behavior.
- Final power, hardware, performance, and other capacity tables remain incomplete until their TODOs are resolved. Recording targets REC-01–04 are now accepted, with a 30-second sudden-power-loss bound at 1 Hz.
- Field firmware installation is currently disabled. Mandatory rollback and development flashing describe intended release behavior.
- External-service support is a requirement with an unfinished release matrix, not a claim that provider integrations exist.

Additional source observations in this revision: `firmware/obc-formats/src/track.rs` defines 20-byte recording samples; `firmware/obc-app/src/recorder.rs` defines the checkpoint and staged-sample constants; `firmware/obc-app/src/ride.rs` distinguishes 128 internal entries from 32 visible entries; `firmware/obc-render/src/font_data.rs` defines the Latin glyph range. New requirements REQ-282–335, excluding retired REQ-321, primarily derive from the owner's feedback, not from a claim of existing code support. The approved ordinary-route recovery change is tracked in issue #1860.

## What belongs outside requirement text

Rationale belongs in a rationale field. Exact screen layout, button bindings, and transitions belong in interaction specifications unless the choice itself is a product constraint. Codecs, caches, algorithms, and task ownership belong in technical specifications. Test procedures, pass evidence, and issue links belong in verification and trace records. Current defects are not desired behavior.

The acceptance tables provide measurable bounds and scope once their TODOs are filled. They do not prescribe the algorithm or reproduce every implementation constant.

## Draft checks

Document checks cover the requirement IDs, shall statements, decision and table IDs, local links, Markdown table structure, and whitespace/newlines. A final review checked the accepted scope, retired tunnel warning, encoding limits, recovery policy, and remaining TODO criteria. No code or public documentation page was changed.

Checks completed for this revision:

- An inline Python check verified 357 unique active requirement IDs covering 001–358 except retired 321, a shall statement in every entry, 27 groups, definitions for all 23 decision references, 112 unique active table criterion IDs, excluding retired NAV-06, local document links, and whitespace/newline integrity.
- The final inline check also verified all four documents, including the editing README, and consistent column counts in each Markdown table.
- `git diff --cached --check` verified the staged handoff files without whitespace diagnostics.

Earlier per-file `git diff --no-index --check` checks also found no whitespace diagnostics. Firmware builds, application tests, formatting, resource measurements, UI snapshots, and the public-site build are deliberately omitted: no application code or public documentation page changed, and those checks cannot verify proposed requirements or resolve product choices.
