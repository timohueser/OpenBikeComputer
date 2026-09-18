# System requirements — acceptance targets

This file supplies the targets referenced by [requirements-draft.md](requirements-draft.md). Decisions and rationale are in [review-notes.md](review-notes.md).

**TODO** means a value, scope, or test condition still needs a decision. **PROPOSED** means a suggested starting value awaiting owner approval. Neither is a passing result. Resolve the affected rows before treating a requirement as a release gate. Table IDs identify acceptance criteria, not additional standalone system requirements.

## Recording and capacity

The recording loss, duration, and minimum stored-ride targets are accepted requirements, not claims about current implementation. Capacity assumes sufficient free supported storage after the map and required update reserves. Continuous duration does not imply that the battery alone lasts that long.

| Criterion | Required capability or bound | Conditions and status |
| --- | --- | --- |
| REC-01 | No accepted samples lost after a successful orderly finish or shutdown | Confirmed intent. Success must follow durable storage, not just a queued write. |
| REC-02 | At most 30 seconds of accepted samples lost after sudden power removal | One position sample per second; supported healthy storage; normal maximum recording workload. Physical power-cut verification required. Other position cadences: TODO bound. |
| REC-03 | At least 72 hours in one recording | One position sample per second, supported sensors and normal navigation; sufficient external power and storage. Include pauses and position outages in verification. |
| REC-04 | At least 30 saved rides of 12 hours each | One position sample per second, all rides accessible for review, export, and deliberate deletion; no phone sync required to retain them. |
| CAP-01 | Maximum installed map bytes and required free replacement space: TODO | Define space for the old map, incoming map, rider data, and update reserve. |
| CAP-02 | Maximum selected map area and corridor length/width: TODO | Include dense urban and mountain areas; area alone does not bound map bytes. |
| CAP-03 | Maximum points, distance, and geographic extent in one route: TODO | Separate application input limit from device route limit; include long trips and loops. |
| CAP-04 | Maximum imported waypoints and waypoint-name length: TODO | Keep first waypoints in route order; warn before omitting later ones. |
| CAP-05 | Maximum saved routes, trips, and stages per trip: TODO | Every item within the declared limit must remain accessible. |
| CAP-06 | Maximum saved ride count and bytes: TODO | Must satisfy REC-04; warn before storage exhaustion. |
| CAP-07 | Supported storage medium, capacity, minimum free space, and write behavior: TODO | Select real release hardware; specify the supported full-storage and fault behavior. |
| CAP-08 | Maximum input file size, points before reduction, and processing memory: TODO | Specify phone, desktop, and browser limits separately where necessary. |
| CAP-09 | Maximum article length and supported photo dimensions/size: TODO | Reading and image loading must remain bounded and cancellable. |
| CAP-10 | Place and landmark search ranges, visible result counts, and paging limits: TODO | Current nearby landmark radius is 10 km. It is a source observation, not a newly approved limit. |

At the current 20-byte sample size, 72 hours at 1 Hz uses 5,184,000 sample bytes, about 4.94 MiB. Thirty 12-hour rides use 25,920,000 sample bytes, about 24.72 MiB. These figures exclude metadata, filesystem overhead, maps, and reserves. The current ten-second checkpoint request is not proof of the accepted 30-second loss bound. The current visible ride catalog holds 32 entries; the required minimum of 30 does not require promising that all 128 internal catalog slots are already visible.

## Import and map content

| Criterion | Required behavior or coverage | Status or conditions |
| --- | --- | --- |
| MAP-01 | Europe, United States, Canada, Australia, New Zealand | Required geographic minimum. Exact country/territory list: TODO. Europe includes more than the EU. Alaska, Hawaii, islands, and dependencies need explicit entries in the final manifest. |
| MAP-02 | Remaining North America | Mexico, Central America, Caribbean, Greenland, and other territories: TODO owner scope. New Zealand is confirmed in MAP-01. |
| MAP-03 | Road classes, local roads, cycleways, and tracks/trails | Required essential map features. They must be distinguishable at the relevant viewing scale when source data exist. |
| MAP-04 | Rivers, lakes, coastlines, settlements and names, forests and open land | Required essential geographic context. Required source categories and scale thresholds: TODO. |
| MAP-05 | Buildings at close scale, bridges and tunnels, contours, service places | Required essential features. Contours retain their independent on/off control; service categories follow REQ-077. |
| MAP-06 | Trail blazes | Required toggle. Supported source markings, symbols, color combinations, overlapping trails, and map-scale behavior: TODO. Turning this layer off must preserve ordinary roads and the active route. |
| MAP-07 | Character support without wider map records | Keep fixed field widths and existing variable-text byte caps; do not duplicate native/transliterated names solely for this feature. The current service/summit name field remains 24 bytes within a 64-byte record. Use a readable bounded fallback when needed; never split a UTF-8 character. |
| IMP-01 | Join disconnected segments only in an unambiguous order and direction | Required. Examples that establish clear order versus ambiguous order: TODO. File order alone is not automatically proof of intended continuity. Show added straight connections in the preview. |
| IMP-02 | Reduce routes above the device point limit before transfer | Required. Maximum geometric deviation: TODO metres. Preserve endpoints, route order, and supported waypoint associations. Verify sharp bends and loops; refuse conversion if the tolerance cannot be met. |
| IMP-03 | Warn before dropping excess waypoints from the end | Required. Use route order; keep the retained count and omitted count visible. For reversal, apply the cap to the final direction sent to the device. |
| IMP-04 | Select one course/track from multi-course/track input | Required. Show enough name, shape, and distance information to distinguish the choices; cancel must preserve the local library. |

“Essential map features” means the types of geographic objects shown on the map. It does not mean another list of application features. Source completeness varies; a missing source object must not be reported as proof that the object does not exist. Additional scripts do not require a global encoding change: summit names and landmark/article text already use UTF-8. Fixed record sizes can remain unchanged. Actual variable-length payloads or compressed output may still differ with their content; byte-identical total map size is not promised.

## Personal estimates and measured accuracy

The owner accepts personal pace learning with mean absolute percentage error below 10% for moving-time predictions on unseen rides after 300 km of prior usable history. Detailed validation conditions remain TODO. No generic estimator is an approved fallback, including in route previews or the Assistant.

| Criterion | Required result | Open conditions |
| --- | --- | --- |
| ETA-01 | Error strictly below 10% | Accepted: mean absolute percentage error of predicted moving time on unseen rides, evaluated per rider and reported separately by supported riding category. Report remaining-time predictions and initial whole-route predictions. This is not a promise that every estimate is within 10%. |
| ETA-02 | Learn from 300 km of the rider's prior usable riding | Accepted threshold. Usable-history criteria and coverage across riding categories: TODO. Do not train on later parts of the ride used to score a prediction. |
| ETA-03 | Define what time is predicted | Accepted: predict moving time and label arrival as excluding future breaks. |
| ETA-04 | Validate useful prediction horizons | TODO: initial whole-route estimate, in-ride checkpoints, minimum remaining duration, and treatment of near-zero denominators. Score per ride before combining so long rides do not dominate merely by supplying more samples. |
| ETA-05 | Demonstrate performance across the advertised scope | TODO: rider count, history coverage, road/gravel/MTB/touring groups, gradients, long rides/fatigue, confidence level, and minimum test set. Report each supported group, not only one pooled average. |
| ETA-06 | Suppress estimates before sufficient learning and outside validated conditions | Required. TODO: eligibility rules and handling of a change in bike/profile or unavailable route inputs. |
| ETA-07 | Recheck the release claim after estimator changes | Required acceptance policy. Use independent held-out data; publish metric and supported conditions with the evidence. |
| ACC-01 | Distance calculation on known input tracks | TODO: tolerance in metres and percent; include stops, loops, dropouts, jumps, and every supported position cadence. Compare against a declared mathematical reference. |
| ACC-02 | Distance on a reference course | TODO: tolerance, course lengths, reference measurement and uncertainty, speed ranges, GNSS reception conditions, and repetitions. Separate open-sky from obstructed-course results. |
| ACC-03 | Recorded ascent on reference courses | TODO: absolute and relative tolerance, elevation reference, filtering scale, minimum course ascent, weather/pressure conditions, and repetitions. Specify what counts as ascent before comparing totals. |
| ACC-04 | No artificial movement or ascent | TODO: stationary duration and allowable drift; include terrain-reference adjustments and GNSS interruptions. |
| CLIMB-01 | Climb detection | TODO: minimum length, minimum average grade, gap tolerance that keeps one climb together, and crest hysteresis. Applies to REQ-177–179. Proposed in the 16 September review. |
| ACC-05 | Position and heading quality | TODO: horizontal-error percentile, compass error, speed at which travel heading is usable, freshness, mounting/calibration conditions, and reacquisition behavior. |

Distance and ascent can have measurable acceptance criteria without a worldwide absolute-accuracy promise. An independently measured course or higher-quality reference trace supplies a reference with stated uncertainty. Replaying a trace checks calculation behavior; it does not prove the accuracy of the receiver or barometer outdoors. Another consumer bike computer is a comparison device, not an absolute reference.

## Power and charging

REQ-308 requires the final device to meet these criteria after the TODO values are set. Each runtime profile must name battery capacity and age, temperature, ride and overnight hours, GNSS cadence, map/navigation workload, display lighting, sensors, phone connection, and transfer activity.

| Criterion | Required result | Target and test conditions |
| --- | --- | --- |
| PWR-01 | Normal-mode riding runtime | At least TODO hours under profile TODO. |
| PWR-02 | Power-saver riding runtime | At least TODO hours under profile TODO; position quality must still meet its declared bounds. |
| PWR-03 | Multi-day use | At least TODO days, each with TODO riding hours and TODO overnight state; battery and workload TODO. |
| PWR-04 | Powered-on idle drain | At most TODO energy or percentage per TODO hours; radios and display state TODO. |
| PWR-05 | Powered-off drain | At most TODO energy or percentage per TODO days; temperature and battery age TODO. |
| PWR-06 | Charging time | At most TODO hours from TODO charge level to TODO level, with supply and device workload TODO. |
| PWR-07 | External input envelope | Nominal regulated 5 V input; allowed voltage, current, ripple, and protection limits TODO. Raw dynamo AC is not a supported input. |
| PWR-08 | Dynamo interruptions | Continue without resets or recording loss for interruption patterns TODO while battery energy is sufficient; approved regulator and restart behavior TODO. |
| PWR-09 | Low-energy warning | At least TODO remaining operating time under profile TODO; warning threshold and battery-estimate uncertainty TODO. |
| PWR-10 | Controlled shutdown reserve | Enough energy for worst-case pending save and shutdown; threshold, workload, and storage timing TODO. |
| PWR-11 | Firmware-update reserve | Minimum charge/external-power conditions TODO; sufficient energy for installation and recovery under defined interruption cases. |
| PWR-12 | Charging protection | Cell voltage, current, and temperature limits TODO from the selected cell and charger; fault and recovery conditions TODO. |
| PWR-13 | Battery aging | Retain at least TODO capacity after TODO cycles and storage exposure; test conditions TODO. |
| PWR-14 | Charge and power-state reporting | Charge-percentage error and state-change delay at most TODO; establish performance across battery age and temperature TODO. |
| PWR-15 | Idle shutdown | Power off after TODO minutes without recording, navigation, or input; rider control (interval choice or disable) TODO. Never during an active or paused ride. Proposed in the 16 September review (REQ-344). |

## Physical hardware and service

REQ-311 requires these acceptance criteria. Do not claim a protection rating until the matching construction and test have been selected and verified.

| Criterion | Required result | Target and test conditions |
| --- | --- | --- |
| PHY-01 | Readable in full sunlight | Illumination, reflections, mounting, displayed examples, and pass procedure TODO. |
| PHY-02 | Readable in complete darkness | Lighting range, glare, contrast, and pass procedure TODO. |
| PHY-03 | Color-accessible presentation | Distinguishable route, track, paths, warnings, and controls in the default UI or enabled accessibility mode. Required color-vision cases and practical inspection TODO. |
| PHY-04 | Rain and spray resistance | Intensity, direction, duration, port-cover state, and pass conditions TODO. |
| PHY-05 | Dust resistance | Particle exposure, duration, port state, and permitted ingress TODO. |
| PHY-06 | Immersion resistance | Depth, duration, water conditions, port state, and pass conditions TODO. |
| PHY-07 | Exposure while charging | Required rain/water exposure, cable and connector configuration, and allowed use TODO. A closed-port result does not establish this capability. |
| PHY-08 | Operating temperature | Minimum and maximum TODO; include display, GNSS, storage, and battery behavior. |
| PHY-09 | Storage temperature | Minimum, maximum, and exposure duration TODO; data retention and post-exposure function required. |
| PHY-10 | Charging temperature | Minimum and maximum TODO from final cell/charger limits; suspend and resume behavior TODO. |
| PHY-11 | Humidity and condensation | Exposure profile, recovery period, and functional checks TODO. |
| PHY-12 | Drop resistance | Height, surface, orientations, number of drops, and permitted damage TODO. |
| PHY-13 | Vibration and shock resistance | Road/off-road exposure profile, duration, mount configuration, and permitted motion/damage TODO. |
| PHY-14 | Mount retention and handling | Mount interface, retention force, attachment/removal force, and service life TODO. |
| PHY-15 | Size and mass | Maximum dimensions and mass, with the included battery and mount scope TODO. |
| PHY-16 | Connector and button life | At least TODO cycles under loads and exposures TODO; function and sealing afterward TODO. |
| PHY-17 | Owner service | Replaceable battery, storage, seals, and mount parts: TODO per part; tools, replacement parts, and instructions TODO. |
| PHY-18 | Sealing after service | Required replacement seals and checks after opening TODO; restored protection level TODO. |
| PHY-19 | Auditory warning output | Audible off-route cue under representative riding noise; sound level, pattern, distance, and conditions TODO. Both off-route and active-route sharp-turn warnings require sound. |

Glove types, wet-hand operation, viewing angles, and reading distances are deferred by the owner. They are not additional release commitments hidden in this table.

## Platforms and delivery

“Recent” establishes the intended platform families but needs an exact release matrix. Browsers are required on the operating systems where the browser is available; this is not a requirement to run Safari on Windows or Linux.

| Surface | Required platform families | Values and distribution decisions |
| --- | --- | --- |
| PLAT-01 — Companion | iOS | Minimum/maximum tested versions, iPhone models, and distribution method TODO. No Android application is added to scope. |
| PLAT-02 — Desktop | macOS | Recent versions, processor architectures, packaging and distribution TODO. |
| PLAT-03 — Desktop | Windows | Recent versions, architectures, USB setup, packaging and distribution TODO. |
| PLAT-04 — Desktop | Linux | Distributions, versions, architectures, USB permissions, packaging and distribution TODO. |
| PLAT-05 — Browser | Firefox, Chrome, Safari, Edge | Recent tested versions and OS combinations TODO. Ordinary website/map-builder functions required; direct device transfer only where the needed browser APIs are available. |
| PLAT-06 — Languages | English, German, French, Spanish | Required across device, companion, desktop, and public website/map builder. Content-language fallback remains REQ-119. |
| PLAT-07 — Geographic names | Names in release regions | Normalization, Romanian Ș/ș/Ț/ț, and native modern Greek/Cyrillic are required within existing map text budgets. Use a readable fallback within the same budget where needed. Exact release repertoire and fallback examples: TODO; policy is settled. |

## Response, completion, and cancellation times

REQ-312 applies to every row. All numerical values are **TODO**. Set both a typical target and a maximum acceptable bound where useful; do not confuse immediate acknowledgement with a completed result. Name release hardware, maximum content sizes, concurrent recording/sensors, cold/warm caches, radio conditions, and external-service conditions for each test. External outages must produce a bounded retryable state, not a false completion guarantee.

| Criterion | Action and measured outcome | Target |
| --- | --- | --- |
| PERF-01 | Power on to usable Home and map | Typical TODO; maximum TODO; cold/warm and recovery cases TODO. |
| PERF-02 | First GNSS fix and reacquisition after loss | Cold, warm, and interrupted cases TODO; reception conditions TODO. |
| PERF-03 | Button input to visible feedback | Maximum TODO, including background work. |
| PERF-04 | Open menu, drawer, settings, or statistics | First usable frame TODO; complete values TODO. |
| PERF-05 | Map pan, zoom, and return to rider | Feedback TODO; complete requested view TODO. |
| PERF-06 | Open route list, overview, and elevation profile | First usable result TODO; full supported route TODO. |
| PERF-07 | Start, pause, resume, finish, and save recording | Feedback TODO; confirmed state/save TODO; no lost accepted data. |
| PERF-08 | Nearby/along-route place search and paging | First result TODO; completion TODO; cancellation TODO. |
| PERF-09 | Calculate detour, visit, easier route, rejoin, or route to start | Feedback TODO; preview TODO; no-route/failure bound TODO; cancellation TODO. |
| PERF-10 | Open What's next, climb, and landmark views | First usable result TODO; completion TODO. |
| PERF-11 | Peak View and change of direction | First useful panorama TODO; completed view TODO; exit/cancellation TODO. |
| PERF-12 | Open article, page text, and open photo | First content TODO; complete image TODO; exit TODO. |
| PERF-13 | Discover/reconnect phone and sensors; zero a supported power meter | Discovery/reconnection TODO; unavailable-device timeout TODO; zeroing completion timeout TODO. |
| PERF-14 | Import, select, simplify, and reverse routes on applications | Feedback TODO; completion TODO at supported input limits; cancellation TODO. |
| PERF-15 | Build/download a map | Local feedback TODO; measured service target TODO; retry/timeout TODO. |
| PERF-16 | USB map/route/ride/firmware transfer | Minimum throughput TODO; progress update interval TODO; cancellation/disconnect handling TODO. |
| PERF-17 | Bluetooth route/ride/firmware transfer | Minimum throughput TODO; progress update interval TODO; cancellation/disconnect handling TODO. |
| PERF-18 | Export and view archived rides on applications | Feedback TODO; completion TODO at supported ride limits. |
| PERF-19 | Firmware validation, installation, trial start, and rollback | Each phase maximum TODO; failure/recovery timeout TODO. |
| PERF-20 | Delete, clean up, factory reset, and shut down | Feedback TODO; completion TODO; interruption outcome per related requirements. |
| PERF-21 | Open emergency location | Maximum TODO actions and TODO time to coordinates or explicit acquiring state. Time to a fix is PERF-02. |
| PERF-22 | External-service export/sync | Queue feedback TODO; progress/check interval TODO; provider timeout/retry bounds TODO. |

## Navigation and warning conditions

| Criterion | Required condition or interaction | Remaining values |
| --- | --- | --- |
| NAV-01 | Position is current enough for route changes and warnings | Maximum age and quality threshold TODO. |
| NAV-02 | Establish off-route state and sound the cue | Distance, persistence time, speed, hysteresis, repeat and re-arm rules TODO; distinguish GNSS loss. |
| NAV-03 | Explicit Back on route request | Rejoin-point policy TODO; prefer forward progress only when it preserves intended remaining route. Preview must expose what will be skipped. |
| NAV-04 | Route to start | Destination is the first point of the selected route. Transition into following that route remains TODO under D21. |
| NAV-05 | Sharp-turn warning | Active route only; derive the turn from its geometry; sound and visual indication required. Turn geometry, approach speed, valid-position gate, and minimum look-ahead TODO. |
| NAV-07 | Warning stability and conflicts | Re-arm distance/time, duplicate suppression, priorities, muting/settings, and false-positive/false-negative acceptance scenarios TODO. |
| NAV-08 | Emergency coordinates | PROPOSED: labelled WGS 84 latitude/longitude in decimal degrees; decimal precision TODO. Last-known age and no-fix state required. Coordinate-format choice TODO. |
| NAV-09 | Navigation after restart | Explicit Resume navigation for both ordinary routes and Assistant journeys after validating sources and fresh position; independent of recording recovery. Never silently accept an unvalidated saved journey. |

NAV-06 is retired with the tunnel-warning requirement. Sharp-turn verification must use named examples of supported active-route geometry, including loops, noisy tracks, and imported straight-line gaps. Absence of a warning is not evidence that a road is free of hazards. Detection must respect route geometry and valid position; the product must not claim complete hazard coverage.

## External-service support

REQ-324 makes external export required. “Support as many services as possible” is a direction for later additions; a release needs a finite list that can be verified. These are initial named targets, with feasibility and exact delivery paths still to be settled.

| Service criterion | Intended support | Open release choices |
| --- | --- | --- |
| SYNC-01 — Strava | Export completed rides; direct integration where provider access permits | TODO: provider application access, host application, authorization flow, direct/manual fallback, automatic upload option, and supported fields. |
| SYNC-02 — intervals.icu | Export completed rides; direct integration where provider access permits | TODO: host application, authorization flow, direct/manual fallback, automatic upload option, and supported fields. |
| SYNC-03 — Additional providers | Add named services where useful and technically supported | TODO prioritized list. Do not label an unavailable direct integration as implemented because generic GPX export exists. |
| SYNC-04 — Existing local rides | Explicit upload/retry with per-service status | TODO: bulk historical upload scope and how a rider opts in. Enabling future automatic uploads must not silently publish the full library. |
| SYNC-05 — Local deletion and reset | Respect the scope of each device, phone, and service action | Device reset does not erase phone archives or remote activities. Disconnection does not erase remote activities. |

Strava documents asynchronous activity uploads in its [upload API](https://developers.strava.com/docs/uploads/). Intervals.icu documents a public API with activity support and OAuth2/API-key authentication on its [integrations page](https://www.intervals.icu/features/app-integrations/). These support investigating direct integrations; they do not establish application approval or a working OpenBikeComputer integration.

## Device route-coverage notices

REQ-359–361 apply to received routes and saved routes selected for riding. These are device-only information notices; they do not add a transfer-protocol requirement or block route use.

| Criterion | Required behavior | Conditions and open details |
| --- | --- | --- |
| COV-01 | Check the route path against installed detailed map coverage | Include sections between route points, gaps inside the coverage, and routes with both endpoints inside coverage but an intervening section outside it. A coverage boundary alone must not hide internal gaps. Geographic coverage does not establish routing access or source completeness. |
| COV-02 | Notify on receipt and again on route start | Use the route-received popup after storage. On selection for riding, check against the current map and briefly show limitations without an additional acceptance. Notice duration and coverage-check response bound: TODO. Preview browsing alone does not trigger the start notice. |
| COV-03 | Distinguish unavailable or unverified coverage | Include no map, unreadable map, and an incomplete check. Do not describe these as confirmed coverage. The information must not prevent storage, selection, navigation, or recording. |

## Routing-profile policies

REQ-362–364 apply to every device-calculated route, including visits and their continuations, detours, easier routes, Back on route, and Route to start. Imported route geometry is not a calculated-route suitability guarantee. Profile choices below remain TODO; the requirement to define and enforce them is accepted.

| Criterion | Policy to define for Road, Gravel, MTB, and Touring | Required distinction |
| --- | --- | --- |
| ROUTE-01 | Permitted and excluded connections; preferences between permitted choices | Exclusions are mandatory. Preferences must not override an exclusion when no permitted route is found. Per-profile choices: TODO. |
| ROUTE-02 | Mapped bicycle access, travel direction, barriers, and conditional restrictions | Define applicable restrictions and the treatment of unknown access or unresolved conditions. Per-profile eligibility rules: TODO. |
| ROUTE-03 | Surface and trail difficulty | Define permitted classes and preferences separately, including how unknown surface or difficulty affects eligibility. Per-profile choices: TODO. |
| ROUTE-04 | Steps, pushing, and carrying a bicycle | Define permitted and excluded cases for each profile. Disclose known required pushing/carrying and steps before route acceptance. Per-profile choices: TODO. |
| ROUTE-05 | Ferries | Define eligibility and disclose ferry sections before acceptance. Mapped existence does not establish current operation or a departure time. Per-profile choices: TODO. |
| ROUTE-06 | Suitability information before acceptance | Identify known affected sections and restricted or uncertain access. Do not treat missing surface/difficulty data as confirmation of suitability. Presentation and section grouping: TODO. |

These policies use available source data. They do not guarantee that mapped access or conditions match the physical route on the day of travel.

## Operation after failures

REQ-239 and REQ-365–367 require the minimum behavior below. Dependencies named in each row must remain available; the table does not require a function to operate without its inputs. Existing accepted-data preservation and recovery limits still apply.

| Criterion | Failure condition | Minimum available behavior |
| --- | --- | --- |
| FAIL-01 | GNSS unavailable | Map browsing, saved-route inspection, and ride controls remain accessible when storage and display work. Live position is unavailable; no fabricated movement or current emergency position. |
| FAIL-02 | Barometer or compass unavailable | GNSS navigation and position recording continue when their inputs remain valid. Dependent values are unavailable or use the already specified fallback; unknown heading follows REQ-280. |
| FAIL-03 | Recording writes fail, including full storage | The unresolved recording failure remains indicated. Navigation, map viewing, and emergency location continue when their data remain available. Any navigation change requiring a failed write must retain its existing failure behavior. |
| FAIL-04 | Map or storage unreadable | Emergency coordinates remain available with working GNSS. Physical controls and a path to the applicable map/storage replacement or recovery operation remain accessible. Do not imply that unreadable map or route data remain usable. |
| FAIL-05 | Application unresponsive | Physical restart remains possible without external equipment. Existing recording and navigation recovery rules apply afterward. Control action, hold duration, and restart response bound: TODO. |
| FAIL-06 | Repeated application startup failures | Provide access to a recovery state without indefinite restart. Do not erase rider data without explicit confirmation. Failure count/window, successful-start definition, and recovery entry conditions: TODO. Firmware recovery remains subject to REQ-354. |

## Independent local tools

REQ-369–372 require an alternative to project-operated services and graphical applications. They do not promise automatic compatibility with every future operating system. Reproducible setup, documented interfaces, and small dependencies must allow the tools to be maintained or ported independently.

| Criterion | Required capability | Conditions and open details |
| --- | --- | --- |
| LOCAL-01 | Rehost map production and the map builder | Publish project source, deployment instructions, and external data dependencies. Do not depend on a private project-only service. Supported setup and resource needs: TODO. |
| LOCAL-02 | Build an installable map locally | Build from documented obtainable source data or locally available building blocks without project-operated services. Include required map-associated data under REQ-008. Input preparation, supported systems, and resource needs: TODO. |
| LOCAL-03 | Transfer maps, routes, and rides through local USB tools | Install a map, upload a route, download a saved route, and download a recorded ride without a graphical application or browser runtime. Preserve applicable transfer validation, interlocks, and data protection. Supported systems and input/output formats: TODO. |

Optional external ride services can require their own provider authorization under REQ-325. They must not become a prerequisite for device functionality or local import/export. Lost-device replacement and library restoration are not added to scope.
