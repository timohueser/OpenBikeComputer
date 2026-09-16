# OpenBikeComputer system requirements — working draft

Prepared 16 September 2026. Product source reviewed: `5ae466a96c6f32e68a2dd78670e9980bd0a4cbb0`. Later changes on remote `develop` were checked for changes to product scope.

This working draft incorporates the owner's scope decisions and is ready for manual review and entry into the requirements system. It does not claim implementation or passing test results. Explicit TODO acceptance values remain to be set. The IDs identify entries independently of their category.

The system includes the bike computer, iOS companion, desktop application, browser map builder, and map and firmware delivery services. Requirements name a particular component where support differs. The simulator, development tools, and requirements console are not rider-facing parts of this system.

The draft includes the owner's scope decisions from 16 September 2026 and excludes weather. Decisions and remaining questions are in [review-notes.md](review-notes.md). Numerical targets, support matrices, and test conditions are in [acceptance-targets.md](acceptance-targets.md). **TODO** means an unresolved value or policy, not an accepted default. **PROPOSED** identifies a suggested value awaiting owner review. A requirement with either marker in its acceptance criteria is not a complete release gate. New entries retain new IDs within the relevant group. REQ-321 is retired after tunnel warnings were removed from scope; its ID is not reused.

## Maps and map management

- **REQ-001 — Coverage selection.** The map builder shall let the rider select a named region, a drawn area, or a corridor around an imported GPX route.
- **REQ-002 — Combined coverage.** The rider shall be able to combine selected areas into one map and remove individual areas before building it.
- **REQ-003 — Route corridor.** The rider shall be able to change the width of a route corridor and preview its coverage before adding it to the selection.
- **REQ-004 — Coverage preview.** The builder shall show the resulting detailed and overview coverage before the rider builds the map.
- **REQ-005 — Coverage gaps.** The builder shall identify known missing and partially covered areas. A failed availability check shall not be reported as confirmed coverage.
- **REQ-006 — Size information.** The builder shall show the expected download size before preparation and distinguish a pending or estimated size from a confirmed size.
- **REQ-007 — Selection limits.** The builder shall reject a selection that exceeds a supported limit and identify the limit that was exceeded.
- **REQ-008 — Associated data.** The normal build flow shall include available routing, terrain, service-place, landmark, and peak content for the selected coverage without separate rider-managed downloads.
- **REQ-009 — Content gaps.** The builder shall disclose known missing data that limit a map feature. Missing optional articles or photos shall not prevent use of the remaining map.
- **REQ-010 — Region joins.** Where the source data connect, joining regions shall preserve road connections and feature identities across their boundaries without adding gaps or duplicate visible features.
- **REQ-011 — One map.** The builder shall deliver the selected coverage and its associated data as one installable map file.
- **REQ-012 — Preparation progress.** The builder shall identify the current preparation stage and show progress where it can be measured.
- **REQ-013 — Preparation recovery.** The rider shall be able to cancel preparation or retry a failed preparation without recreating the coverage selection. An incomplete result shall not be offered as ready to install.
- **REQ-014 — Map appearance.** The builder shall let the rider preview and select a supplied or locally saved custom map appearance before creating the map. Appearance changes shall not change routing access rules or geographic data.
- **REQ-015 — Map information.** The rider shall be able to inspect map coverage, available source-date information, and source attribution. A build date shall not be presented as the age of the source data.
- **REQ-016 — Installation.** The desktop application and a supported browser shall let the rider send a newly prepared map or an existing supported map file to the device over USB.
- **REQ-017 — Replacement notice.** Before transfer, the sending application shall state that installation replaces the active map and whether a restart is needed.
- **REQ-018 — Insufficient storage.** If the map cannot be installed with the available storage, the device shall refuse the transfer without deleting the previous map and the application shall report insufficient storage.
- **REQ-019 — Transfer progress.** During map transfer, the application and device shall indicate that transfer is active. The application shall show the transferred and total size.
- **REQ-020 — Transfer cancellation.** The sending application shall let the rider cancel a map transfer. Cancellation shall preserve the previous map.
- **REQ-021 — Interrupted replacement.** A transfer failure, disconnection, or power loss before successful installation shall not activate a partial map or invalidate the previous map.
- **REQ-022 — Map validation.** The system shall check transferred content for damage and supported map structure before reporting that installation succeeded.
- **REQ-023 — Transfer retry.** After a failed or cancelled transfer, the rider shall be able to resend the map without manually removing partial data. Restarting the transfer from the beginning is permitted.
- **REQ-024 — Map activation.** After successful installation and the requested restart, the device shall use the new map for display and all map-dependent features.
- **REQ-025 — Other stored data.** Replacing a map shall preserve saved routes, rides, and settings. Selections that depend on the old map shall be checked again before use.
- **REQ-026 — Offline maps.** Installed map functions shall work after a restart without internet access or a connected phone.
- **REQ-027 — Map browsing.** The rider shall be able to browse the installed map without loading a route or starting a recording.
- **REQ-028 — Position following.** With a valid fix, the map shall follow the rider's position while following mode is active.
- **REQ-029 — Map inspection.** The rider shall be able to pan and zoom the map. Live position and heading changes shall not move or rotate a manually inspected view.
- **REQ-030 — Return to position.** One action shall leave manual map inspection and restore position following when a valid fix is available.
- **REQ-031 — Route inspection.** With a route loaded, the rider shall be able to inspect positions along that route without changing the route or recording state.
- **REQ-032 — Map interpretation.** The map shall distinguish the rider, active route, recorded track, and route waypoints. The rider shall be able to show a distance scale in the selected units.
- **REQ-033 — Map features.** The map shall distinguish the essential geographic features defined in the map-content table in acceptance-targets.md: roads by class, cycleways and trails, water, settlements and names, forests and open land, buildings at close scale, bridges and tunnels, contours, and service places.
- **REQ-034 — Map unavailable.** The device shall distinguish no installed map, an unreadable map, and a location outside available coverage. It shall not present missing coverage as confirmed empty terrain.
- **REQ-035 — Map failure recovery.** A map read failure shall leave ride controls and a means to install a replacement accessible. The device shall indicate when the displayed map is incomplete.

Map management has one active installed map, replaced through USB. Multiple selectable installed maps, phone map transfer, and automatic map updates are outside scope.

- **REQ-282 — Map installation interlock.** The device shall refuse map transfer while a recording is active, paused, or awaiting a save or discard decision. The sending application shall show the reason before starting transfer.
- **REQ-283 — Map display switches.** The rider shall be able to enable and disable contours, the map clock, and the map scale bar independently. Each choice shall survive restart.
- **REQ-284 — Release geography.** The map service shall support map preparation for Europe, the United States, Canada, Australia, and New Zealand within the declared map-size limits. The exact country and territory manifest, including the remainder of North America, remains TODO in acceptance-targets.md.
- **REQ-285 — Trail blazes.** The rider shall be able to show or hide mapped trail blazes independently of the active route. The display shall identify the marked trail without implying that the rider's selected route follows it.
- **REQ-286 — Trail-blaze source.** Trail blazes shall use available source markings and associations. Missing or unsupported markings shall not be replaced by an invented blaze. Supported marking types remain TODO in acceptance-targets.md.

## Route import and trip management

- **REQ-036 — Phone import.** The iOS companion shall accept GPX and TCX route files from the file picker and the system share action.
- **REQ-037 — Computer import.** The desktop application and browser device page shall accept GPX route files for transfer to the device.
- **REQ-038 — Import preview.** Before sending a route, the application shall show its name, track shape, distance, and available elevation information.
- **REQ-039 — Route content.** Import shall preserve route direction, available elevation, supported authored waypoints, and geographic shape within the approved simplification tolerance. Import limits and tolerances shall be stated in acceptance-targets.md.
- **REQ-040 — Waypoint identity.** Supported waypoint names, order, and categories shall remain available on the device. A waypoint with no recognized category shall remain a generic waypoint.
- **REQ-041 — Off-route annotations.** An authored waypoint beside the route shall remain an annotation at its supplied location. Import shall not silently change the route to pass through it.
- **REQ-042 — Invalid input.** Import shall report an unreadable or unsupported file without presenting a partial route as complete. Supported point reduction, waypoint removal, and single-track selection shall follow the explicit import rules below.
- **REQ-043 — Name collisions.** When a phone import has the same name as a saved route, the companion shall let the rider replace that route, save a separately named route, or cancel.
- **REQ-044 — Local library.** The companion shall let the rider import an already available local file, save the route, and rename it without an internet or device connection.
- **REQ-045 — Device copy status.** The companion shall distinguish a locally saved route from a confirmed device copy and identify when local changes have not been sent.
- **REQ-046 — Route transfer.** The companion shall send routes over Bluetooth. The desktop application and supported browser shall send routes over USB.
- **REQ-047 — Transfer outcome.** A route shall be reported as on the device only after the device confirms storage. A failed or cancelled replacement shall preserve the previous stored route.
- **REQ-048 — Received route.** A successfully received route shall appear in the device route list without restarting the device.
- **REQ-049 — No unsolicited ride change.** Receiving a route shall not start, end, or replace a ride without the rider's choice. Dismissing a received-route prompt shall keep the route available in the list.
- **REQ-050 — Route overview.** The device shall show a route's name, shape, distance, available elevation profile, ascent, descent, and available time estimate before the rider starts it.
- **REQ-051 — Leave preview.** Leaving a route preview without starting it shall restore the previous navigation selection and shall not start recording.
- **REQ-052 — Trip organization.** The companion shall let the rider create and rename a trip, add or remove route stages, and change their order. Removing a stage from a trip shall not delete the route from the phone library.
- **REQ-053 — Computer trip import.** When several GPX files are selected on the computer device page, the rider shall be able to send them as separate routes or as ordered stages of a named trip.
- **REQ-054 — Trip transfer.** Trip transfer shall preserve stage order. A failed stage transfer shall not be reported as a complete trip transfer.
- **REQ-055 — Device trip browsing.** The device shall present trips as one level of ordered route groups. Each stage shall remain independently selectable. Reaching a stage end shall not start the next stage automatically.
- **REQ-056 — Route deletion.** Device route deletion shall require a deliberate confirmation and shall not delete an active route. A deletion failure shall not be shown as success.
- **REQ-057 — Trip deletion.** Before deleting a trip and its device routes, the device shall state the deletion scope. It shall preserve an active route and all routes outside that scope.

Route reversal is an application function. An on-device reversal function and a route editor are outside scope.

- **REQ-287 — Ordered segment gaps.** Import shall connect disconnected segments with straight lines when their order and direction are unambiguous. The preview shall identify the added connections as imported gaps, not verified mapped riding connections. Ambiguous ordering shall produce a warning and prevent import.
- **REQ-288 — Multiple tracks or courses.** When a file contains several tracks or courses, the application shall warn the rider and let them choose a single track or course to import or cancel. It shall not silently combine or select them.
- **REQ-289 — Point reduction.** When a selected route exceeds the supported point limit, the application shall reduce its point count before device transfer. An information popup shall state the reduction, and the preview shall show the resulting route. Failure to meet the approved shape tolerance shall prevent transfer.
- **REQ-290 — Excess waypoints.** When a route exceeds the supported waypoint limit, the application shall warn the rider, retain the first supported waypoints in route order, and omit the remaining waypoints from the device copy. The warning shall state the retained and omitted counts before transfer.
- **REQ-291 — Route reversal.** The companion and computer route-import applications shall let the rider reverse a route before transfer and preview the result. Reversal shall update travel direction, waypoint order, elevation profile direction, and ascent and descent summaries without changing geographic waypoint locations.

## Navigation and detours

- **REQ-058 — Route following.** The device shall follow a selected route offline and show the rider's position, travel direction, route direction, and progress along it.
- **REQ-059 — Route matching.** At crossings and on routes that pass the same place more than once, route progress shall use the rider's travel history and direction to avoid switching to an unrelated part of the route.
- **REQ-060 — Off-route state.** When the rider leaves the route, the device shall indicate that state and show distance to the route when it can calculate it.
- **REQ-061 — Off-route progress.** While the rider is off-route, the device shall not advance route waypoints or climb completion solely because another route segment is nearby.
- **REQ-062 — Rejoining.** When the rider rejoins the route, guidance shall resume from the matched route position without resetting the recording.
- **REQ-063 — Lost position.** Loss of a valid fix shall be shown separately from an off-route condition. Stale position data shall not be treated as fresh route progress.
- **REQ-064 — Route replacement during a ride.** When the rider selects another route during a ride, the device shall offer to change navigation while keeping the recording, finish the current recording and start a new ride, or cancel.
- **REQ-065 — Recording independence.** Changing the active route shall not reset recorded distance, time, ascent, or sensor totals when the rider chooses to keep the current recording.
- **REQ-066 — Routing profile.** Standard maps shall provide Road, Gravel, MTB, and Touring profiles. The rider shall be able to select an available profile. Planning shall use its access rules and preferences, and the device shall show the effective profile.
- **REQ-067 — Real connections.** A calculated route shall use supported mapped connections. An unmatched origin or destination shall not be replaced by an unlabelled straight-line riding connection.
- **REQ-068 — Detour selection.** The rider shall be able to select a point ahead on the active route and request a detour around the intervening section.
- **REQ-069 — Detour preview.** Before acceptance, the device shall show the calculated detour, the section it replaces, the rejoin point, and the distance difference.
- **REQ-070 — Detour acceptance.** A detour shall change navigation only after explicit acceptance. Cancelling its calculation or preview shall preserve the existing route and recording.
- **REQ-071 — Planning state.** Route calculation shall show that work is in progress and allow cancellation. A late result after cancellation shall not change navigation.
- **REQ-072 — Planning failures.** The device shall distinguish no route found from unavailable map data, unavailable planning resources, and a failure to save the result.
- **REQ-073 — Changed inputs.** A route preview shall not be accepted against a different map, route, bike profile, or materially changed departure position without a new validation and, when the result changes, renewed rider acceptance.
- **REQ-074 — Route completion.** Reaching the route finish shall not discard or automatically finish the ride recording.

After restart, navigation resumes only by the rider's explicit choice. Back on route is a rider-requested Assistant action. It does not authorize automatic rerouting.

- **REQ-292 — Off-route sound.** When the device establishes that the rider has left the active planned route, it shall issue an auditory cue as well as the off-route indication. The trigger, repeat, and re-arm conditions are TODO in acceptance-targets.md.
- **REQ-293 — No automatic rerouting.** Leaving a route shall not automatically calculate or accept a replacement route. The existing planned route shall remain selected until the rider chooses a navigation change.
- **REQ-294 — No turn-by-turn guidance.** The product shall not provide turn-by-turn maneuver instructions. Sharp-turn warnings shall remain distinct from maneuver guidance.
- **REQ-295 — Back on route.** The Ride Assistant shall let the rider explicitly request an offline route from their current valid position back to the active planned route, using the selected bike profile.
- **REQ-296 — Rejoin preview.** Before accepting Back on route, the rider shall see the proposed connection, rejoin location, and remaining planned route. Acceptance shall preserve the recording; cancellation or failure shall preserve the previous navigation. The rejoin-point selection policy is TODO.
- **REQ-297 — Route to start.** The device shall let the rider request and preview an offline route from their current valid position to the first point of the selected route using the selected bike profile.
- **REQ-298 — Start-route acceptance.** Route to start shall change navigation only after explicit acceptance. Failure or cancellation shall preserve the previous navigation and recording. The transition from reaching the start into following the selected route is TODO under D21.

- **REQ-096 — Navigation recovery.** After a restart, the device shall offer Resume navigation for the saved ordinary route or Ride Assistant journey only after checking its map and route sources and a fresh, unambiguous position. Acceptance shall restore the navigation plan and its applicable progress or journey phase. Declining shall leave guidance inactive and preserve the route files.
- **REQ-097 — Separate recovery choices.** Resuming navigation shall not start recording. Continuing or discarding a recovered recording shall not silently accept or delete the saved navigation plan.

## Ride Assistant: places and visits

- **REQ-075 — Assistant access.** The rider shall be able to open the Ride Assistant during a ride without pausing recording.
- **REQ-076 — Offline questions.** Find a place, What's next, Landmarks, and Easier route shall use installed offline data without a phone connection.
- **REQ-077 — Place categories.** Find a place shall include water, campsites, accommodation, resupply, pharmacies, bike shops, and train stations. Train-station results shall not imply timetable information.
- **REQ-078 — Search context.** Without an active route, place search shall find nearby places. With an active route, it shall support places relevant to the remaining route.
- **REQ-079 — Result access.** More places shall give access to further results in the selected category. The number of visible cards shall not be presented as the total number of available places.
- **REQ-080 — Search completeness.** The device shall distinguish a completed search with no matches from a partial, failed, or unavailable search.
- **REQ-081 — Stable browsing.** Position updates shall not silently change the selected place or reorder the result page while the rider browses it. Refresh shall start a new search.
- **REQ-082 — Place details.** Place details shall show the available name, category, location, and opening information. Any displayed straight-line distance shall be distinguishable from a calculated route distance.
- **REQ-083 — Opening state.** Search results for usable services shall exclude places confirmed closed now. Places with unknown hours may remain, but shall not be labelled open.
- **REQ-084 — Unknown local time.** Missing or unsupported schedules, an untrusted clock, or an unknown local offset shall produce unknown opening status rather than a guess.
- **REQ-085 — Visit costs.** A visit preview shall show the actual route to the place, distance and ascent to arrival, and the added distance and ascent of the complete visit and continuation when those values are available.
- **REQ-086 — Complete visit plan.** Before accepting a visit from an active route, the device shall prepare both the path to the place and the continuation back to the remaining journey.
- **REQ-087 — Authored stops.** Visit and alternative-route planning shall preserve the remaining authored waypoint order and required accepted stops. If it cannot do so, it shall refuse the change rather than silently omit a stop.
- **REQ-088 — Preview independence.** Searching, reading place details, and previewing a visit shall not change navigation or start recording.
- **REQ-089 — Place-route acceptance.** Explicitly accepting a route to a place shall start recording if no ride session exists. It shall preserve the current recording if a ride session already exists.
- **REQ-090 — Acceptance checks.** Before accepting a place route, the device shall recheck position, map, bike profile, mapped access, and current opening eligibility. A materially changed plan shall require a new preview and acceptance.
- **REQ-091 — Visit arrival.** On arrival at an accepted visit, guidance shall switch once to the accepted continuation without requiring a new route calculation. An arrival message shall not pause guidance or recording.
- **REQ-092 — Current visit.** The rider shall be able to reopen the current visit, inspect its current leg, and leave that view without cancelling the visit.
- **REQ-093 — Cancel before departure.** Cancelling a visit before departure shall restore the original route only after that change has been saved successfully.
- **REQ-094 — Cancel after departure.** Cancelling a visit after departure shall require preview and acceptance of a mapped return connection. Leaving that preview shall keep the accepted visit.
- **REQ-095 — Failed visit change.** A failed save of a visit change shall preserve the last confirmed journey state and allow retry. Closing a view while a save is pending shall not be reported as cancellation of the save.
- **REQ-098 — Generated routes.** Temporary Assistant routes shall not appear as rider-imported entries in the saved Routes list. Accepting one shall preserve the imported source route.

## Ride Assistant: What's next and easier routes

- **REQ-099 — Ahead window.** What's next shall offer 5 km and 10 km route windows and display distances in the selected units.
- **REQ-100 — Ahead overview.** The overview shall show the selected interval's elevation profile, ascent and descent, next-climb facts, next authored waypoint, and next mapped water and resupply opportunity when available.
- **REQ-101 — Authored meaning.** The device shall keep the author's waypoint name and shall not infer that an uncategorized waypoint is lunch, rest, or accommodation.
- **REQ-102 — Beyond-window waypoint.** If the next authored waypoint lies beyond the profile window, its displayed distance shall make that position clear.
- **REQ-103 — Ahead timeline.** Explore ahead shall expose all available results in the selected interval in route order, with along-route distance and any relevant lateral offset.
- **REQ-104 — Ahead filters.** The rider shall be able to filter the timeline by place category and by authored waypoints, mapped places, or both. These filters shall not silently change the overview.
- **REQ-105 — Return from details.** Leaving a timeline detail shall restore the selected result and list position.
- **REQ-106 — Accepted journey.** During a visit, ahead information shall include the accepted return leg and remaining journey. Arrival at the visit shall not be treated as the journey finish.
- **REQ-107 — Unknown route facts.** Missing elevation or surface data shall be shown as unavailable information, not zero ascent, flat terrain, or a smooth surface.
- **REQ-108 — Easier-route goals.** Easier route shall offer separate searches for less climbing, smoother surface, and shorter distance.
- **REQ-109 — Real improvement.** An offered alternative shall improve its stated goal, retain the required destination and stops, and obey the selected bike profile.
- **REQ-110 — Alternative comparison.** The device shall show the current route and selected alternative on a stable map, followed by a comparison of available distance, ascent, and surface costs before acceptance.
- **REQ-111 — Explicit alternative acceptance.** Only Use this route shall accept an easier route. Back shall preserve the selection or return to the prior view without changing navigation or recording.
- **REQ-112 — Unavailable alternatives.** Easier route shall be unavailable during an active visit or when an accepted blockage cannot be preserved. A failed search shall not offer invented or repeated alternatives.
- **REQ-113 — No unsupported time claim.** A shorter or smoother alternative shall not be described as faster unless a supported time estimate establishes that claim.

## Landmarks and offline articles

- **REQ-114 — Nearby landmarks.** Landmarks shall show installed physical sites within the supported nearby search area in nearest straight-line order, with a stable map and selected card. The release search radius shall be specified in acceptance-targets.md.
- **REQ-115 — Landmark scope.** The landmark collection shall include approved physical-site categories and passes. It shall exclude peaks, lakes, glaciers, industrial sites, and settlements as landmark entries.
- **REQ-116 — Landmark paging.** More landmarks shall reach further results. Refresh shall repeat the search from the current position. Distinct sites at the same coordinates shall remain separate entries.
- **REQ-117 — Offline reading.** The rider shall be able to read the installed article for a selected site without an internet connection. Longer text shall be available through reading pages.
- **REQ-118 — Optional photos.** Where an installed photo is available, the rider shall be able to view it. A missing or unreadable photo shall not prevent reading the article.
- **REQ-119 — Article language.** The device shall select installed text in the chosen device language, then English, then the map's selected fallback language.
- **REQ-120 — Language changes.** Changing the device language shall select another installed article version without requiring a new map build or download.
- **REQ-121 — Sources.** The rider shall be able to inspect the selected article's source and available photo attribution. Leaving Sources shall restore the selected site and reading page.
- **REQ-122 — Reading closed sites.** A landmark's article shall remain readable when the site is closed. Current opening eligibility shall govern Visit, not article access.
- **REQ-123 — Unmapped access.** A site with no mapped approach allowed by the selected bike profile shall remain readable, but shall not offer a visit route that implies verified access.
- **REQ-124 — Correct associations.** The installed article and photo shall belong to the selected site. Missing or damaged references shall not open content for another site.

## Peak View

- **REQ-125 — Offline panorama.** Peak View shall use installed terrain and summit data to display a panorama from the rider's available position without an internet connection.
- **REQ-126 — Live direction.** In Live mode, the panorama shall follow the current heading. Rotating the view at one position shall not change its vertical scale.
- **REQ-127 — Peak selection.** The rider shall be able to enter Browse, select visible peaks, and inspect their names, elevations, and distances.
- **REQ-128 — Frozen inspection.** Browse shall preserve its observer position, viewing direction, and selected peak while the rider inspects the panorama or reads linked content.
- **REQ-129 — Panorama progress.** While the panorama is being prepared, completed areas shall remain visible and incomplete areas shall be marked. Turning shall give the new viewing direction priority.
- **REQ-130 — Terrain gaps.** Peak View shall distinguish pending terrain, missing distant terrain, and unavailable observer terrain. Missing terrain shall not be shown as confirmed open visibility.
- **REQ-131 — Leaving Peak View.** The rider shall be able to leave Peak View during preparation and return to ordinary device use without changing the ride recording.
- **REQ-132 — Peak articles.** A selected peak shall show a content indicator only when readable installed article content is available. The rider shall be able to open its text, optional photo, and Sources.
- **REQ-133 — Peak return path.** Back from peak content shall restore the same selected peak and Browse direction. Back from Browse shall return to Live before leaving Peak View.
- **REQ-134 — Separate peak content.** Peak articles shall be accessible through Peak View and shall not appear as Ride Assistant landmarks or visit destinations.

Peak View does not establish visibility through vegetation, buildings, or weather.

## Ride recording and recovery

- **REQ-135 — Route-free recording.** The rider shall be able to start recording without selecting a route.
- **REQ-136 — Recording state.** The device shall distinguish recording, paused, saving, saved, and failed recording states so the rider can determine whether samples are being stored.
- **REQ-137 — Recorded content.** A recording shall preserve accepted position samples, sample timing, available elevation, available sensor measurements, and segment boundaries.
- **REQ-138 — Pause and resume.** Pausing shall stop recording new ride samples and accumulating ride totals. Resuming shall continue the same recording with a new segment.
- **REQ-139 — Moving statistics.** Moving time and average speed shall exclude stopped intervals. Excluding stopped time shall not by itself finish or discard the recording.
- **REQ-140 — Position gaps.** A position outage, rejected position jump, pause, or restart shall not add a straight-line travel segment across the missing interval or count that interval as measured movement.
- **REQ-141 — Recording continuity.** Opening menus, changing views, browsing places, accepting a navigation change, or losing a phone or external sensor connection shall not reset the recording.
- **REQ-142 — Finish.** Finishing shall save all accepted pending samples and the ride summary before reporting the ride saved.
- **REQ-143 — Failed save.** If final saving fails, the device shall retain the recoverable recording, show the failure, and allow retry rather than silently discard it.
- **REQ-144 — Discard.** Discarding a recording shall require a deliberate confirmation and remove only the selected recording. A failed discard shall remain visible as a failure.
- **REQ-145 — Fresh session.** Starting a new ride after saving or discarding shall create a separate recording with fresh totals and shall not inherit a prior recording error as a new failure.
- **REQ-146 — Restart recovery.** After an interrupted recording, the device shall offer to continue the recovered ride or deliberately discard it. Continuing shall preserve its saved samples, start time, and accumulated totals.
- **REQ-147 — Recovery boundary.** Recovery shall restore data through the last valid saved checkpoint without creating samples for the power-off interval. Lost accepted samples shall remain within the recording-loss limit in acceptance-targets.md.
- **REQ-148 — Damaged recording.** A damaged recording shall be identified as damaged. Any offered repair or removal shall affect only the exact recording confirmed by the rider.
- **REQ-149 — Recording failure.** Storage exhaustion or a write failure shall produce a visible recording warning. The device shall not continue to claim that the ride log is complete when samples could not be saved.
- **REQ-150 — Saved rides.** The device shall list saved rides and let the rider inspect their available track, elevation profile, and summary before deletion.
- **REQ-151 — Manual ride deletion.** Deleting a saved ride on the device shall require deliberate confirmation. A saved ride shall not be deleted automatically because it is old or has been archived elsewhere.

## Ride transfer, archives, and export

- **REQ-152 — Phone sync.** The companion shall let the rider explicitly download completed device rides. Reconnecting alone shall not start downloading all missing rides.
- **REQ-153 — Complete archive.** Before reporting a ride saved locally, the companion shall store its summary and complete verified samples together in persistent storage.
- **REQ-154 — Source identity.** Sync shall distinguish rides from different devices, replaced or reformatted storage, and different revisions of the same ride. These sources shall not overwrite one another by matching a name or short identifier.
- **REQ-155 — No duplicate sync.** Repeating sync of the same verified ride shall not create a second local ride. A changed source shall not be mistaken for the previously saved copy.
- **REQ-156 — Interrupted batch.** If a multi-ride sync fails, completed local archives shall remain saved. The application shall report partial progress and let the rider continue the remaining work.
- **REQ-157 — Archive confirmation.** The device shall show a ride as archived only after receiving and saving proof of a verified persistent copy of that exact ride.
- **REQ-158 — Confirmation retry.** A lost archive-confirmation response shall not require a second local archive. Reconnect or explicit retry shall reconcile the saved copy and the device's confirmation.
- **REQ-159 — Confirmation failure.** If the local copy is saved but device confirmation fails, the application shall distinguish those two outcomes and keep the local archive.
- **REQ-160 — Export.** The companion and computer applications shall let the rider export a completed ride as GPX without deleting it from the device.
- **REQ-161 — Export fidelity.** GPX export shall preserve recorded coordinates, available elevation, available heart rate, cadence and power, and pause or outage segment boundaries. Missing measurements shall not be exported as zero.
- **REQ-162 — Export timing.** GPX export shall preserve valid recorded sample timestamps and omit unavailable timestamps rather than invent them.
- **REQ-163 — Download is not archive proof.** A browser download alone shall not mark a ride as safely archived on the device.
- **REQ-164 — Computer archive.** A desktop ride archive shall remain usable if its exported GPX file is moved or removed. The rider shall be able to export the archived ride again.
- **REQ-165 — Local ride review.** A locally archived ride shall remain viewable and exportable without the device connection. Unavailable online map tiles shall not hide the saved track and summary.
- **REQ-166 — Phone trash.** Deleting a ride from the companion library shall place it in Recently Deleted for 30 days. The rider shall be able to restore it or deliberately delete it permanently during that period.
- **REQ-167 — Respect local deletion.** Sync shall not silently restore rides that the rider deleted or placed in Recently Deleted on the phone.
- **REQ-168 — Deletion scope.** A deletion action shall state whether it affects the local library, the device, or both. Deleting one copy shall not silently remove other copies outside that scope.

## Ride statistics, elevation, and climbs

- **REQ-169 — Ride fields.** Statistics shall offer speed, moving average speed, ridden and remaining distance, recorded and remaining ascent, route grade, current elevation, moving time, and clock time when their required data are available. Time remaining and arrival time shall be offered only when the personal-estimate requirements are met.
- **REQ-170 — Place and sensor fields.** Statistics shall also offer next waypoint, upcoming waypoint list, next water, campsite, accommodation, resupply, pharmacy, bike shop, heart rate, power, and cadence fields.
- **REQ-171 — Field selection.** The rider shall be able to add, remove, and reorder statistics fields. A selection spanning several pages shall remain accessible through the configured page cycle.
- **REQ-172 — Missing values.** A field with missing or stale input shall show an unavailable value instead of a fabricated zero or a frozen value presented as current.
- **REQ-173 — Units.** Distance, speed, and elevation shall use the selected metric or imperial units consistently across ride views and summaries.
- **REQ-174 — Route profile.** The statistics view shall show the available route elevation profile and the rider's matched progress. The rider shall be able to inspect the profile without changing navigation.
- **REQ-175 — Elevation reference.** With suitable installed terrain and valid position data, the displayed altitude shall use the terrain reference to correct long-term barometer offset while retaining measured short-term height changes.
- **REQ-176 — Recorded ascent.** Correcting the displayed altitude reference shall not add artificial ascent to the recording or rewrite recorded heights as map measurements.
- **REQ-177 — Climb facts.** For a detected active route climb, the device shall show its elevation profile, rider progress, remaining distance, remaining ascent, current route grade, and average grade to the top.
- **REQ-178 — Climb modes.** The rider shall be able to turn the climb screen off, reach it manually, or allow it to appear automatically on climb entry and return to the map at the crest.
- **REQ-179 — Climb interruptions.** Automatic climb presentation shall not interrupt a confirmation, an active input gesture, or an unrelated screen the rider is using. Position noise at a climb boundary shall not repeatedly switch views.
- **REQ-180 — Personal time estimates.** Every predicted ride duration, remaining time, and arrival time on any product surface shall use a model learned from the rider's own recorded rides. A fixed bike-profile estimate shall not substitute for personal learning.
- **REQ-181 — Arrival time.** Arrival clock time shall be shown only when a validated personal remaining-moving-time estimate and trusted local time are available. The presentation shall state that future stops are excluded. Missing elevation shall not be presented as evidence that the remaining route is flat.
- **REQ-182 — Waypoint display.** The rider shall be able to hide next-waypoint notices, show them on approach, or show them whenever a named waypoint remains ahead.

Personal pace learning is required product work. The current research and bike-profile estimator do not establish compliance. The accepted benchmark uses moving-time mean absolute percentage error below 10% after 300 km. Detailed validation conditions remain **TODO** under **D13**.

- **REQ-299 — ETA release threshold.** Time-estimation features shall be included in a product release only after the personal model demonstrates mean absolute percentage error below 10% for moving-time predictions on unseen rides after 300 km of prior usable rider history. Validation shall evaluate each rider and report supported riding categories and long rides separately. The detailed sampling and validation conditions are TODO in acceptance-targets.md.
- **REQ-300 — Insufficient learning.** When the rider has insufficient usable history or the current journey is outside the validated conditions, predicted duration and arrival values shall be unavailable. The interface shall explain the unavailable estimate without substituting an unvalidated generic estimate.
- **REQ-301 — Learned pace continuity.** The system shall retain the rider's learned pace across ordinary restarts and learn from valid recorded movement without treating pauses, missing positions, or sensor failures as measured pace. The learning state shall be removed by factory reset.
- **REQ-302 — Unreferenced altitude.** When barometric altitude has not obtained a valid elevation reference, the device shall distinguish it from referenced elevation in its displayed status.
- **REQ-303 — Distance and ascent accuracy.** Recorded distance and ascent shall meet the reference-course and input-replay accuracy criteria in acceptance-targets.md. Those criteria shall state the reference method, measurement uncertainty, position cadence, and terrain and reception conditions; values are TODO.

## External sensors

- **REQ-183 — Sensor support.** The device shall receive heart rate, cycling power, and cadence from supported Bluetooth Low Energy sensors while maintaining its phone connection. These are the required external sensor quantities; ANT+ is outside product scope.
- **REQ-184 — Sensor selection.** The rider shall be able to discover, select, and forget one saved sensor for each supported quantity from the device.
- **REQ-185 — Sensor reconnection.** The device shall retain saved sensor choices across restart and reconnect when the selected sensor is available and Bluetooth is enabled.
- **REQ-186 — Sensor status.** The device shall distinguish an unset sensor from searching, connecting, and connected states and show reported sensor battery level when available.
- **REQ-187 — Stale measurements.** A sensor value more than five seconds old shall become unavailable in live display and recording. A reported zero cadence shall remain distinguishable from missing cadence.
- **REQ-188 — Cadence source.** A selected cadence sensor shall take priority over cadence from a power meter. A power meter may supply cadence when no separate cadence sensor is selected.
- **REQ-189 — Sensor summaries.** Ride sensor averages shall use only intervals with valid measurements during moving time. Missing data shall not lower the average as zero samples.
- **REQ-190 — Optional sensors.** Failure or absence of an external sensor shall not prevent navigation or position recording.

Wheel-speed sensors, ANT+, training zones, normalized power, and training-load metrics are outside scope. Power-meter zeroing is required when the meter exposes the operation over a supported Bluetooth Low Energy interface.

- **REQ-330 — Power-meter zeroing.** The device shall offer Zero power meter for a connected power meter that exposes the operation over a supported Bluetooth Low Energy interface. The action shall explain the required unloaded preparation and require an explicit rider request. It shall not offer an unsupported operation as available.
- **REQ-331 — Zeroing outcome.** After a zeroing request, the device shall show progress and report success only when the power meter confirms it. Meter-reported failure, disconnection, and timeout shall produce an explicit failed or unknown result and permit a deliberate retry.

## Phone and USB connectivity

- **REQ-191 — Pairing.** Pairing a new phone shall require a passkey shown on the device. The system shall distinguish pairing failure from an ordinary connection loss.
- **REQ-192 — Phone ownership.** The device shall retain one paired phone across restarts and refuse replacement by another phone until the rider forgets the existing pairing.
- **REQ-193 — Protected phone access.** Device data transfer and phone configuration writes over Bluetooth shall require the authorized paired connection.
- **REQ-194 — Reconnection.** A previously paired phone shall be able to reconnect without repeating pairing. Link loss shall leave the device's navigation and recording running.
- **REQ-195 — Forget phone.** The rider shall be able to forget the phone on the device. The application shall explain any operating-system pairing removal needed before pairing again.
- **REQ-196 — Radio control.** The rider shall be able to disable and enable Bluetooth on the device. Disabling it shall disconnect the phone and sensors without deleting their saved identities.
- **REQ-197 — Connection information.** Applications shall identify the connected device and its firmware version and shall not show a disconnected device as ready for writes.
- **REQ-198 — USB use.** USB transfer shall work without Bluetooth pairing. Disconnecting USB shall not leave the device stuck in transfer mode.
- **REQ-199 — Incompatible peers.** An application shall identify unsupported device protocol or content versions before performing a write that requires them.
- **REQ-200 — Device changes.** After reconnect, storage replacement, or reformatting, applications shall refresh device contents before applying operations based on an old listing.
- **REQ-201 — Conflicting operations.** Simultaneous requests shall not interleave file content or apply a command to the wrong object. A blocked request shall wait, be cancelled, or fail visibly.
- **REQ-202 — Phone-independent operation.** Loss of the companion application, phone permissions, or phone internet access shall not disable already installed device functions.

## Controls, settings, language, and time

- **REQ-203 — Device controls.** All rider-facing device functions shall be operable through the four physical buttons without a touchscreen or phone.
- **REQ-204 — Deliberate destructive actions.** Finish, discard, delete, reset, and power-off actions shall require the intended confirmation. Releasing a hold before completion shall not perform the action.
- **REQ-205 — Input isolation.** Opening a drawer or completing a button combination shall not also trigger the underlying screen's action. A new prompt shall not consume a hold already in progress as confirmation.
- **REQ-206 — Return path.** Every ordinary browsing view shall provide a way back without changing navigation or recording. A blocked recovery or transfer view shall explain how to leave or complete it.
- **REQ-207 — Quick controls.** The quick drawer shall provide Bluetooth control, settings access, power-off, and brightness control when the hardware supports it.
- **REQ-208 — Context controls.** Feature-specific settings and secondary actions shall be accessible from the relevant view. Unavailable actions shall not appear usable.
- **REQ-209 — Brightness.** The rider shall be able to preview and choose the supported brightness levels. Cancelling the brightness editor shall restore the previous level.
- **REQ-210 — Idle return.** The rider shall be able to choose the idle-return interval or disable it. Idle return shall lead to the map during an active ride and Home otherwise, without accepting a pending confirmation.
- **REQ-211 — Release languages.** The device, iOS companion, desktop application, and public website including the map builder shall support English, German, French, and Spanish at release. Each language selector shall display each language in its own name.
- **REQ-212 — Readable content.** Labels, values, and actions shall remain distinguishable in each supported language and unit system. Essential values shall not be hidden by overlapping text or clipped controls.
- **REQ-213 — Persistent preferences.** Confirmed device preferences shall survive a normal restart. Unreadable preferences shall produce defined defaults rather than invalid controls or a startup failure.
- **REQ-214 — Factory reset.** After explicit confirmation, factory reset shall restore device preferences to defaults and delete all stored files and rider state except the installed map. Deletion shall include routes, trips, recordings, temporary and update files, recovery state, learned pace data, saved sensor identities, and phone pairing credentials. Installed firmware shall remain usable.
- **REQ-215 — Device name.** The companion shall let the rider name the device and shall distinguish a locally entered name from a device-confirmed change.
- **REQ-216 — Clock sources.** The device shall obtain UTC time from valid GNSS or the paired phone. A stored time from an earlier boot shall not by itself establish a trusted current time.
- **REQ-217 — Local time.** The rider shall be able to set the local UTC offset, including quarter-hour offsets. Opening-status decisions shall require both trusted UTC and an established local offset.
- **REQ-218 — About information.** The rider shall be able to inspect device identity and installed firmware information from the device.

The four release languages apply across the user interfaces. Native Greek/Cyrillic support is required within existing map text budgets, with a readable fallback where needed. The exact release repertoire and fallback examples remain TODO; full Unicode support is not assumed.

- **REQ-304 — Color accessibility.** The device shall let a rider with impaired color discrimination distinguish primary map features, navigation state, warnings, and available actions through the default presentation or a persistent accessibility mode in settings. The inspection cases are TODO in acceptance-targets.md.
- **REQ-305 — Reset scope notice.** Before factory reset, the device shall state that the map is retained and all other device rider data, preferences, learning, and pairings are removed. The notice shall distinguish device deletion from copies held on the phone or an external service.
- **REQ-306 — Reset completion.** Factory reset shall report completion only after the reset scope is applied. If reset is interrupted or a deletion fails, the next startup shall report the incomplete reset and allow completion without falsely showing a clean device. A completed reset shall require new phone pairing.
- **REQ-307 — Geographic text.** Names in the declared release regions shall remain readable under the device character policy, including text normalization, Romanian letters, and supported Greek and Cyrillic characters. Unsupported or over-limit names shall use the defined readable fallback without becoming strings of replacement marks.

- **REQ-332 — Text normalization.** Text prepared for device display shall use precomposed Unicode characters where available and readable equivalents for unsupported typographic punctuation. Normalization shall preserve supported letters and names. Display normalization shall not overwrite original names in application libraries or exported source data.
- **REQ-333 — Romanian characters.** All device text sizes used for names and prose shall render the Romanian letters Ș, ș, Ț, and ț in addition to the existing supported Latin characters.
- **REQ-334 — Greek and Cyrillic characters.** All device text sizes used for names and prose shall render the modern Greek and Cyrillic repertoire defined for the release regions. This support shall apply to names preserved by map preparation and route import, subject to the map text limits.
- **REQ-335 — Compact map text.** Additional character support shall retain existing fixed map-name field sizes, record widths, and text byte limits. It shall not require stored duplicate native and transliterated names. Where a native name cannot be represented readably within its limit, preparation shall use a consistent readable transliteration or shortened name within the same limit. Stored text shall end on complete characters.

## Power and charging

This group includes proposed final-hardware behavior. The development board does not establish battery or charging acceptance.

- **REQ-219 — Battery indication.** The device shall show available battery state and a low-battery indication. Unavailable battery measurement shall not be presented as a measured charge percentage.
- **REQ-220 — Position interval.** The rider shall be able to select the supported position update interval. Changing it shall not reset the ride or silently discard every interval longer than the default cadence.
- **REQ-221 — Power saver.** The rider shall be able to enable and disable GNSS power saving without ending the recording. Its position-quality and timing limits shall be defined before release.
- **REQ-222 — Idle operation.** When no active function requires GNSS, the device shall reduce GNSS activity. Opening a function that needs the current position shall request position acquisition independently of recording.
- **REQ-223 — Stable display.** An unchanged screen shall remain visible without repeated full-screen redraws solely to keep it displayed.
- **REQ-224 — Shutdown.** An orderly shutdown shall preserve recoverable active-ride data before power is removed and shall not report a failed save as successful.
- **REQ-225 — External power.** The final device shall remain usable for navigation and recording while powered and charged from its supported external power input.
- **REQ-226 — Power transitions.** Connecting or disconnecting external power shall not reset the device or interrupt a recording while sufficient battery power remains.
- **REQ-227 — Dynamo supply.** The final device shall support the agreed regulated 5 V dynamo supply conditions without repeated resets or loss of saved data. The permitted supply interruptions and current limits remain open in **D17**.
- **REQ-228 — Charging protection.** The final device shall keep battery charging within the selected cell's permitted voltage, current, and temperature limits and suspend charging when those limits cannot be met.
- **REQ-229 — Charging state.** The final device shall distinguish charging, external power without charging, full charge, and charging failure when the hardware can establish those states.
- **REQ-230 — Low-energy operation.** Before energy is too low for continued recording, the final device shall warn the rider and preserve recoverable ride data. The warning margin and shutdown threshold remain open in **D17**.

Battery runtime, standby drain, charging duration, and critical-energy values are explicitly **TODO** in acceptance-targets.md. Runtime conditions must include battery age, temperature, GNSS mode, lighting, sensors, and phone activity.

- **REQ-308 — Power acceptance.** The final device shall meet every power and charging criterion in the Power and charging table in acceptance-targets.md. Each unresolved target and test condition is explicitly TODO.

## Storage and fault handling

- **REQ-231 — No automatic data erasure.** A full, missing, unrecognized, or damaged storage card shall not cause automatic formatting or deletion of saved rider data.
- **REQ-232 — Route cleanup.** When route cleanup is offered, the rider shall select an age and explicitly confirm deletion. Cleanup shall preserve active routes, rides, maps, and routes whose last-use age is unknown.
- **REQ-233 — Trustworthy cleanup age.** Age-based cleanup shall require a trusted clock and shall not use a failed metadata read as evidence that a route is old.
- **REQ-234 — Partial cleanup.** If cleanup stops after some deletions, the device shall report that outcome without claiming that all requested cleanup completed.
- **REQ-235 — Formatting.** Formatting from a supported application shall state which device data will be erased and require explicit confirmation. It shall not be offered as automatic repair for one damaged recording.
- **REQ-236 — Unrelated data.** A failed create, replace, or delete operation shall not corrupt unrelated stored objects.
- **REQ-237 — Uncertain writes.** If the system cannot determine whether a write completed, it shall reconcile the stored result before retrying a conflicting change or reporting success.
- **REQ-238 — Malformed content.** Unsupported or malformed map, route, trip, article, photo, or update content shall produce a bounded failure rather than an uncontrolled restart or an unrelated-data change.
- **REQ-239 — Missing hardware.** Failure of GNSS, barometer, compass, or storage shall be reported for the affected function. Functions that do not require that component shall remain accessible where the device can continue operating.

## Firmware updates

These are intended release requirements. Device installation is currently disabled by board policy; package upload is not proof of installation support.

- **REQ-240 — Update discovery.** Supported companion and computer applications shall let the rider check for a newer released firmware version and inspect its release information before installation.
- **REQ-241 — Release channels.** Normal update checks shall use stable releases. Prerelease firmware shall require an explicit opt-in, and an unrecognized development version shall not trigger a guessed automatic upgrade.
- **REQ-242 — Local update package.** The rider shall be able to select a compatible local update package without requiring a live release-service connection during device installation.
- **REQ-243 — Package integrity and authority.** The device shall reject damaged, incomplete, unsupported, or unauthorized update packages before replacing running firmware.
- **REQ-244 — Separate upload and install.** Uploading a package shall not install it. A remotely requested installation shall require confirmation on the device.
- **REQ-245 — Installation conditions.** The device shall refuse installation while a ride session is active, when available power is insufficient, or when the required installation and recovery storage cannot be reserved.
- **REQ-246 — Update progress.** The device and application shall distinguish download, transfer, validation, installation, and final outcome. A transfer success shall not be described as an installation success.
- **REQ-247 — Before firmware replacement.** A failure before firmware replacement begins shall leave the previous firmware usable.
- **REQ-248 — Interrupted installation.** A supported power interruption during firmware replacement shall leave a recoverable installation state that can complete installation or restore the retained firmware when power and required storage return.
- **REQ-249 — Trial failure.** If new firmware does not complete its required startup confirmation, the device shall restore the retained previous firmware rather than repeatedly boot the failed trial.
- **REQ-250 — Installed version.** After a successful update and reconnect, the device shall report the installed version so the application can confirm the result.
- **REQ-251 — Rider data during updates.** A normal supported firmware update shall preserve rider data declared compatible with that release. Required destructive preparation or incompatible stored formats shall be disclosed before installation. This does not require support for all historical prerelease formats.

Normal field updates require a retained rollback image and device confirmation. Background update checks and notifications remain **TODO** under **D18**. Firmware release tooling has its own requirements in the release console project.

- **REQ-309 — Mandatory rollback reserve.** Before normal field installation begins, the device shall retain and verify a recoverable copy of the previous firmware. Failure to obtain that copy shall block installation.
- **REQ-310 — Owner firmware.** The project shall provide a documented development flashing procedure for owner-built firmware. This procedure shall remain separate from authenticated public-release updates and shall state any data erasure or recovery steps it requires.

## Physical hardware and service

The processor is an existing project constraint. Environmental, mechanical, lifetime, and service acceptance targets are explicitly **TODO** in acceptance-targets.md. No protection rating or certification is claimed.

- **REQ-252 — Processor constraint.** The device shall use the nRF54LM20 as its main processor.
- **REQ-253 — Outdoor display.** The mounted device shall let the rider read its primary map, navigation state, and ride values in full sunlight and complete darkness, using the supported display lighting as needed. The inspection conditions shall be defined in acceptance-targets.md.
- **REQ-254 — Physical input.** The rider shall be able to operate all required device controls without an attached phone. Glove, wet-hand, viewing-angle, and reading-distance acceptance conditions are deferred.
- **REQ-255 — Weather resistance.** With its ports in the declared riding configuration, the device shall continue operating after the agreed rain, spray, dust, and immersion exposures.
- **REQ-256 — Mechanical retention.** The device shall remain attached and retain its usable viewing position under the agreed road and off-road vibration and shock conditions.
- **REQ-257 — Mount handling.** The rider shall be able to attach and remove the device using the selected mount without damaging the device or requiring access to the enclosure interior.
- **REQ-258 — Temperature.** The device shall operate, store data, and charge only within the separately declared operating, storage, and charging temperature ranges.
- **REQ-259 — Drop resistance.** After the agreed drop exposures, the device shall retain the required functions and shall not expose a battery or electrical hazard.
- **REQ-260 — Service access.** The hardware release shall identify the parts the owner can replace and provide the required access and instructions. Battery, storage, enclosure seals, and mount service scope remain open in **D19**.
- **REQ-261 — Hardware release information.** A released buildable hardware version shall include its schematics, PCB files, parts list, mechanical files, assembly instructions, and matching firmware and programming instructions.

- **REQ-311 — Hardware acceptance.** The final hardware shall meet every applicable criterion in the Physical hardware and service table in acceptance-targets.md. Exposure conditions, limits, and service scope remain explicitly TODO.

## User ownership and privacy

- **REQ-262 — No mandatory account.** Using the device's rider functions, importing routes, building public-data maps, and exporting rides shall not require an OpenBikeComputer account or subscription.
- **REQ-263 — Local ride ownership.** Ride recordings and imported routes shall remain available to the rider without a vendor-hosted personal library.
- **REQ-264 — No silent sharing.** The system shall not upload private routes, rides, or live location to a remote service without an explicit rider action or an enabled feature that states what it sends.
- **REQ-265 — Data removal.** The rider shall be able to delete locally held routes, rides, and pairing information through the relevant device or application controls.
- **REQ-266 — Published source.** Each public product release shall identify the corresponding software source, hardware revision where applicable, and build instructions under the project's declared open-source licenses.

## Supported applications and operating limits

- **REQ-267 — Supported platforms.** The desktop application shall support recent macOS, Windows, and Linux releases. The website and map builder shall support recent Firefox, Chrome, Safari, and Edge releases. The exact OS, browser, and iOS companion versions and available functions shall be specified in acceptance-targets.md.
- **REQ-268 — Unsupported browser functions.** Where a browser cannot perform a required local device operation, the site shall explain that limitation and offer the supported desktop path without claiming the operation succeeded.
- **REQ-269 — Offline libraries.** The companion and desktop applications shall allow access to saved local routes and rides when their internet services are unavailable.
- **REQ-270 — Service failures.** Failure of map or update delivery services shall produce a retryable error and shall not prevent use of already installed maps or firmware.
- **REQ-271 — Published limits.** The product shall state its supported map size, route size, route and ride counts, recording duration, and import limits. Exceeding a limit shall use the specified warning, reduction, selection, or refusal flow without silent data loss.
- **REQ-272 — Responsive controls.** Map loading, route planning, place search, panorama generation, and transfers shall meet the approved response and cancellation times without losing accepted ride samples under the declared operating conditions.
- **REQ-273 — Operation under load.** The supported maximum map, route, and recording workloads shall operate within device memory and storage limits without an uncontrolled restart.

The referenced tables contain incomplete acceptance criteria. Their TODO values must be resolved before the affected requirements can act as release gates.

- **REQ-312 — Action timing.** The supported applications and device shall meet the response, completion, and cancellation limits for the actions in acceptance-targets.md. Limits and test workloads are TODO; an indefinite progress indicator shall not count as successful completion.
- **REQ-313 — Recording capacity.** The device shall meet the continuous-recording and stored-ride capacity criteria in acceptance-targets.md while keeping every ride within the declared capacity accessible for review, export, and deletion.
- **REQ-314 — Accepted-data preservation.** An orderly finish or shutdown shall preserve all accepted recording samples. After sudden power loss at one sample per second on supported healthy storage, accepted-data loss shall not exceed 30 seconds. Bounds for other supported cadences remain TODO in acceptance-targets.md. A storage failure shall be reported separately.

## Position and heading

- **REQ-274 — Independent positioning.** During a ride, the device shall obtain its position from its own GNSS receiver without needing the phone's location or an internet connection.
- **REQ-275 — Acquisition state.** The device shall distinguish acquiring a position, having a valid position, and losing the position. A missing receiver shall not be presented as an established position.
- **REQ-276 — Recovery of position.** After a temporary position outage, receipt of a valid fix shall restore position-dependent functions without requiring a new recording.
- **REQ-277 — Valid input.** Invalid coordinates, invalid fix status, or measurements older than the agreed freshness limit shall not be used as current position. Freshness conditions remain open in **D06** and **D20**.
- **REQ-278 — Moving direction.** When a valid direction of travel is available, the device shall use it for heading-relative ride views.
- **REQ-279 — Stationary direction.** When direction of travel is unavailable and a valid compass heading is available, the device shall use the compass for heading-relative map and Peak View presentation.
- **REQ-280 — No heading.** When neither direction source is available, the map shall use a labelled north-up view without showing an assumed rider heading as measured. Peak View shall indicate unavailable live direction and permit manual direction browsing.
- **REQ-281 — Position inspection while riding.** Browsing another map location, route point, or peak shall not replace the real rider position used for recording and active guidance.

Position accuracy, acquisition time, compass accuracy, and calibration conditions are **TODO** in acceptance-targets.md. A sensor polling interval is not an accuracy guarantee.

- **REQ-315 — Position on demand.** Opening position-following maps, nearby place search, Peak View, or emergency location shall request a current GNSS position even when no ride is recording. Acquisition shall show a waiting state and shall not itself start a recording.

## Emergency location and hazard warnings

Emergency location and route-based sharp-turn warnings are required. Tunnel and other mapped-road hazard warnings are outside the present scope. Remaining interaction choices and numerical targets are TODO in acceptance-targets.md.

- **REQ-316 — Emergency access.** The rider shall be able to open an emergency location screen through a short, documented action from normal device use, including when no ride is recording. The maximum number of actions is TODO.
- **REQ-317 — Emergency coordinates.** The emergency screen shall show readable latitude and longitude, the coordinate format and reference system, and the age and validity of the position. Its information shall remain usable without a map, phone, or internet connection.
- **REQ-318 — No current emergency fix.** When no current position is available, the emergency screen shall show acquisition status. Any displayed last known position shall be labelled with its age and shall not be presented as the rider's current location.
- **REQ-319 — Emergency-screen continuity.** Opening and leaving the emergency location screen shall preserve recording and navigation. Automatic screen changes shall not dismiss it while the rider is using it.
- **REQ-320 — Sharp-turn warning.** While following an active route, the device shall give an auditory cue and a visual warning before a qualifying sharp turn derived from that route's geometry. It shall not issue these warnings without an active route. Geometric thresholds, valid-position conditions, approach speed, and lead distance or time are TODO in acceptance-targets.md.
- **REQ-322 — Warning stability.** Position noise shall not repeatedly issue the same off-route or sharp-turn warning. Warning reset conditions shall permit a later distinct occurrence to be warned again; thresholds are TODO.
- **REQ-323 — Warning interaction.** Warnings shall identify their cause without accepting an open confirmation or resetting the recording. A warning shall not hide emergency coordinates while that screen is open. The priority between simultaneous warnings is TODO.

## External ride services

External ride export and optional synchronization are required. The release service list and application responsibilities are defined in acceptance-targets.md. Additional providers are a product goal until named there.

- **REQ-324 — External-service export.** The system shall support export of completed rides to the approved external services, including Strava and intervals.icu where provider access permits it. For each service, the support matrix shall identify direct integration or a supported file-import path and disclose any missing capability.
- **REQ-325 — Service consent.** Connecting a service and enabling automatic uploads shall require an explicit rider choice for that service. The application shall state which ride data it sends and whether future completed rides will be uploaded automatically.
- **REQ-326 — Service upload outcome.** The application shall distinguish queued, uploading, processing, completed, and failed transfers where applicable. It shall report completion only after the provider confirms creation or acceptance of the activity.
- **REQ-327 — Service retry.** Provider outages, expired authorization, or interrupted uploads shall preserve the local ride and allow retry. Before retrying an uncertain upload, the application shall reconcile the provider result when possible; an unresolved outcome shall be shown instead of claiming success or blindly duplicating the activity.
- **REQ-328 — Service disconnect.** The rider shall be able to disable automatic uploads or disconnect a service. Disconnecting shall clear local authorization credentials and stop further uploads; it shall not silently delete already uploaded activities.
- **REQ-329 — Service data fidelity.** Service exports shall preserve supported timestamps, coordinates, elevation, sensor measurements, and recording gaps. The integration shall document any information that the provider cannot accept and keep the complete local recording available.
