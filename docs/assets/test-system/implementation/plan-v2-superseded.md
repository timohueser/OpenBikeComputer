> **Superseded.** This is the first full proposal, kept because `plan.md` cites it. The
> settled design is `plan.md`; the authoritative scope is epic #1816. Do not implement from
> this file.

# OpenBikeComputer test system — redesign plan (v2, after adversarial review)

Date: 2026-09-13. Base: develop `d42e2e3a`. Evidence: ten parallel surveys of every test
subsystem, one build-time measurement pass on this Mac, a sweep of #1449/#1448 and their children,
and a six-lens adversarial review of the first draft (rules, build feasibility, harness
architecture, LOC honesty, simplest-system, HIL). Every number is measured or counted unless marked
"estimate". LOC convention throughout: git-tracked files, `wc -l`; inline Rust tests are
brace-matched `#[cfg(test)] mod` blocks plus whole test-only source files.

## 0. The short version

The product code already has the seams a system-test layer needs. The device app is one function per
frame: it takes inputs (buttons, GPS fix, sensors, clock, answers from the last frame) and returns a
plan (what to repaint, what work the host must do). The protocol engine that talks to the phone and
the web builder is a `no_std` state machine over an in-memory SD card whose writes can be cut mid-way.
The weather bakery is driven through three traits. The phone and web clients each sit behind one
transport interface.

The test system does not use those seams as a system. The loop that performs the app's requested
work and feeds results back is written six times. The device is faked separately in TypeScript, Swift
and Rust, and only the Rust fake is the real engine. 255 real scenario tests live inside the firmware
crate for private access. Every app edit relinks 22 test binaries, then pays ~40 s of first-launch
cost on this Mac before a single test runs. A 1,654-line selector computes which crates a change
affects, and CI then compiles and runs the whole workspace anyway.

The plan: (1) cut the build fan-out and the CI job shape first, because that is mechanical and
measured; (2) build one small host-side virtual device from the simulator's existing headless
assembly and move the firmware crate's scenario tests onto it; (3) compile the real protocol engine
to WebAssembly so the web tests and the browser dev-harness run against firmware truth instead of a
hand-written fake; (4) replace the suite registry with cargo's own graph plus a ~300-line selection
script; (5) delete about 10k lines of dead or duplicated test code and add the few system tests that
are missing. Net effect: an app-crate edit tests in seconds instead of ~72 s, the CI page becomes
about twenty jobs whose names are the test map (half of them skip on a typical change), and test code
goes down by an estimated 6k to 11k lines while the number of true system tests goes up.

## 1. What we have today

### 1.1 Size (one convention)

| Surface | Tests | LOC | Notes |
|---|---:|---:|---|
| Rust `tests/*.rs` | 1,270 | 53.1k | 118 files, 110 integration binaries across 38 crates |
| Rust inline `#[cfg(test)]` | 2,449 | 58.3k | firmware 35.6k, host 18.4k, apps 4.4k; obc-app alone 25.2k (21.7k inline + 3.5k in-crate harness) |
| Web (vitest) | 796 (972 as the runner counts them) | 15.9k | 67 files; Node + happy-dom, no real browser |
| Swift (OBCKit) | 916 | 19.8k | 92 files |
| XCUITest | 77 methods | 2.5k | 75 of 77 never run in CI |
| Python | 302 | 4.9k | tools, firmware/tools, ops/weather, builder |
| Test infrastructure | — | ~11k | selector 1,654; registry 928; ci.yml 1,171 (33% comments); sweep script 936; guards 805; resource_guard 1,102; tooling tests 3,992 |

About 165k lines of test and test-infrastructure code against about 285k lines of product code.

### 1.2 What is good and stays

- The seams: `App::run_pass(PassInputs) -> PassPlan` with one typed effect slot per domain;
  `HostLoop` + `HostPlatform` + in-memory stores; `obc_link::flat::Engine` over
  `FlatStore<SparseDisk>` with `FaultOnce`/`When` media cuts behind the 28-line `BlockDevice` trait;
  weather `Adapter`/`FixtureUpstream`/`DirStore`; Swift `TransferLink`; TypeScript `DeviceLink`.
- The suites that are the model for everything else: the DeviceCore conformance corpus (24 scenarios
  × 61 requirement rows, run with immediate and with delayed answers, compared on rider-visible state;
  it found two production bugs); the storage crash matrix (every media write cut before, during,
  inside and after, remount must equal the model); `obc-link/tests/flat_engine.rs` (real engine, real
  store, torn card, hand-built phone bytes); `obcm-assemble/tests/oracle.rs` (pack vs cut+assemble
  must render and route identically); `dirty_parity` (render-every-frame vs render-on-demand,
  compared byte for byte, no goldens to update).
- Independent oracles (obcm-testkit hand-writes spec bytes; SyntheticDem; `build_obcr`), the
  content-addressed fixture registry, and the 2026-06 audit's finding that the tests assert real
  invariants rather than tautologies.

### 1.3 The problems, ranked by what they cost you

1. **The inner loop is slow for structural reasons.** An obc-app edit on this Mac: 12.7 s build (the
   22 integration-test units occupy 73% of that timeline), then ~44 s of dead wall while macOS
   validates 23 freshly linked binaries on first launch (0 CPU, ~2 s each; confirmed in two shells,
   please confirm in yours), then 4.7 s of tests, then 10.7 s of `clippy --all-targets`. About 72 s
   per edit. Building only the lib plus one test after the same edit takes 2.9 s. A leaf edit
   (obc-formats) plus a workspace test build: 178 s and 159 executables. Heavy dev-dependencies
   make it worse: obc-app pulls obc-dem with tiff/ureq/rustls/ring (44 crates) for one string
   constant; obc-route pulls obc-pack, and with it libGEOS and 91 crates, because two test files use
   the packer's OBCM writer and nav-graph model.
2. **The system-test layer exists but is scattered and duplicated.** The "perform the app's effects,
   deposit outcomes" loop exists as HostLoop (production), the board's RideExec (production, no host
   test), CoreHarness (582 LOC, conformance test), Planner (260, obc-app support), Sweeper (90,
   app.rs tests) and dirty_parity's drive_replay. Five input-script dialects. About 13 test fakes of
   `LocationSource`, 18 `VecSink`s, 8 read-counting sources, 4 recording draw targets. The device is
   faked in TypeScript (`MockDevice`, ~650 LOC of policy), in Swift (four scripted `TransferLink`
   actors, 368 LOC, plus the 2.7k-LOC `OBCMock` UI device) and in Rust (`flat_harness`, 588 LOC,
   the only one built on the real engine).
3. **255 scenario tests hide inside the firmware crate.** `app.rs` (140 tests, 4,146 LOC) and
   `src/harness/*` (115 tests, 3,529 LOC) build an App, press buttons, run frames and assert on
   screens. They are inline to reach private state: the dominant need is *seeding* (85 writes to the
   navigator's route state to put the rider "on a route at 300 m"), not reading (`progress_m` and
   `off_route` are already public). The price is a `cfg(test)` self-alias in `lib.rs`, a `#[path]`
   include that compiles `support.rs` twice, and 33 `cfg(test)`-only accessors. About 100 of the 255
   are unit tests by any definition (34 screen tests through a hand-built context, 40 machine tests)
   and should simply move next to the code they test.
4. **CI precision dies at the job boundary.** The selector computes which of 41 Rust suites a change
   affects; the `test` job then runs `cargo nextest run --workspace --all-features` regardless. The
   same job unconditionally runs the UI snapshot sweep (233 obc-sim launches, 50 s), a release build
   of obc-bench, and a release build of obc-pack plus a pip install for pytest (28 s). Editing
   CLAUDE.md selects all 25 jobs, including the 13–18 min iOS build. Nine unconditional single-step
   jobs each pay a runner spin-up.
5. **The infrastructure is bigger than the problem.** 41 of 69 registry entries are `rust.<pkg>` with
   the same command template, a copy of cargo's package list. About 770 of the selector's 1,654
   lines exist to prove that three regex walkers over ci.yml agree with the plan; the weather matrix
   is written to satisfy the parser. All 12 `cadence_conflict` entries cite the epic itself; a weekly
   workflow exists only to confirm the epic is still open. Levels `live`/`hardware`, cadences
   `never`/`nightly`/`release`, and `quarantine` have zero users. Four suites declare weekly or manual
   cadences that no workflow runs. Two suites run zero tests. The `fixtures` field is free text nobody
   validates. `coverage-policy.toml` has no tool behind it.
6. **Goldens churn.** `firmware/ui-snapshots.sha256` changed 19 times in 21 days; regenerating it
   means a 936-line bash script launching obc-sim 233 times.
7. **Dead scaffolding survives its purpose.** `tools/s6b_board_cutover_soak.py` (999 LOC, death
   trigger "#1494 closes"; closed 2026-08-26), the retired S0 BLE transfer protocol (~1k LOC of
   production code, ~900 of tests, 8 vectors; no user), `nowcast_skill_events.rs` (875 LOC, 36 s per
   PR, one product assertion), `OBCMockTests` (958 LOC testing the fake), ~60 XCUITest methods that
   duplicate host-model tests and never run, the LOC-ledger scripts and their tests (1,657 LOC) for a
   policy you retired, a board bin for a closed issue, legacy-library compatibility pins in a
   pre-release project.
8. **The missing top of the pyramid** (unchanged since the #1449 audit): nothing runs in a real
   browser; the desktop app is never launched; the phone's transfer client never meets firmware
   truth; no test goes from a packed map through the card to a rendered App frame, or from a baked
   weather product to the device screen; RideRecovery, the one screen only a board reboot produces,
   is not in the sweep.

## 2. Design

### 2.1 Principles that change decisions

- **Cargo is the registry for Rust.** Package graph, reverse dependencies and test targets come
  from `cargo metadata`. Nothing copies that into TOML.
- **Two kinds, three cadences.** A test is either a *unit* test (the crate's lib target) or a
  *system* test (the crate's one `tests/main.rs` target), and cargo already knows which. A test runs
  at one of three cadences: on every affected change (PR), on demand (heavy), or on a bench with a
  board (hardware). Nothing else is a tier, and no field records it.
- **One virtual device, built from production code.** The app-level device is the simulator's
  headless assembly (App + HostLoop + in-memory stores) exported as a library. The protocol-level
  device is the real engine over the torn-able card, compiled to WebAssembly. Fakes that
  re-implement product policy are deleted.
- **System tests assert on what the rider or the phone sees**: screens, frames, effects and
  outcomes, catalog bytes, wire records. Private state belongs to unit tests inside the module.
- **Fewer, bigger binaries.** One PR-tier integration target per crate, plus named heavy targets.
- **Local and CI run the same script.** Each CI job is a script under `tools/ci/`; `obc` calls the
  same scripts.
- **Scaffolding names its consumer** (your rule). Every harness here lists who uses it.

### 2.2 Kinds and cadences

| | unit (lib target) | system (`tests/main.rs`) |
|---|---|---|
| **PR** — every affected change, local and CI | inline `#[cfg(test)]`, vitest, Swift target tests | scenarios over the virtual device, engine-over-torn-card suites, differential oracles, the conformance corpus, the screen sweep, contract/vector pins; hermetic; may read synced fixtures; ≤ 60 s per suite |
| **heavy** — `workflow_dispatch` and release time; never blocks an unrelated PR | — | captured-data bakes (airmass), fuzz loops, memory and throughput measurements, the full XCUITest set, real-browser runs over large inputs |
| **hardware** — a local `obc` recipe run by a person with a board | — | DK over VCOM + USB; phone over BLE (always two separate sessions: the board parks the radio while USB power is present) |

"Contract" is a property of a test (it pins bytes) and no longer a level. "Fixture" is a fact about
a test (it reads a synced package), not a level; a missing package fails loudly with the exact sync
command. The 15-minute weather freshness probe is production monitoring and leaves the test system.

### 2.3 The virtual device

**A. App level: `Device`.** The simulator's headless path already is the virtual device: it drives
`App::run_pass` through the production `HostLoop` with in-memory stores, settles, and renders one
frame. What is missing is a library export. `Device` moves those ~110 lines (stores, headless
platform, settle, scripted input) into `obc-host-core` behind a `testing` feature (the pattern
obc-display already uses for its `conformance` feature, so nothing test-only reaches obc-web-demo's
wasm bundle or obc-skin-preview). Surface: `new(map, support)`, `keys`, `fix`, `settle`, `pass_at`,
`frame`, `top_screen`, `screen_stack`, a `ScriptedPorts` covering the six ports tests actually script
(fix, HR, power, cadence, battery, compass), and two typed seeds on `App` — `riding_on(route,
progress_m)` and `with_recovered_ride(bytes)` — that replace the private pokes. A method is added only
when a second consumer needs it; dirty_parity keeps its own `Step` builder on top.

Consumers: the relocated obc-app scenarios, obc-sim headless rendering, the screen sweep,
dirty_parity, obc-app's `Frames`/`Planner` support. Not a consumer: the conformance corpus. Its
runner delivers answers late on purpose, which HostLoop cannot do without splitting its `execute`
into "serve" and "deliver" (a production change) and which would also turn a policy-free gate into
a HostLoop test. CoreHarness stays; only its duplicated fakes (~100 LOC) shrink. Note that HostLoop
needs a loaded map for every frame, so scenarios that never touch the map still pay a minimal OBCM
build; `build_min_obcm` folds into obcm-testkit.

Where the relocated scenarios live: in obc-host-core's own `tests/` (the crate that owns `Device`).
The alternative, obc-app's `tests/` dev-depending on obc-host-core, is a dependency cycle cargo
accepts for integration tests but which makes every `cargo test -p obc-app` build obc-app twice
plus obc-host-core, on the exact path we are trying to make fast. Consequence to accept:
`obc test -p obc-app` runs only obc-app's unit tests (fast); the scenarios run through
`obc test affected` and CI, selected by reverse dependency whenever obc-app changes.

**B. Protocol level: `obc-flat-device`.** `obc_link::flat::Engine` + `FlatStore<SparseDisk>` with
the sim card's fault knobs, wrapped with wasm-bindgen (~220 LOC of glue) as a fourth small bridge in
`build:wasm`. The TypeScript `loopback.ts` keeps its packet-slicing pipe (a transport property) and
drops `MockDevice`. Knob mapping, checked against the engine: busy, no-space-with-bytes-required,
paged LIST with a staleness-checked cursor, compare-and-swap on replace, bilateral cancel and
link-lost are engine-native; card size and formatted/unformatted come from `SparseDisk::blank` and
`flat_harness`; record ceilings from `Ceilings`; `armPolicy: allow` needs a small `Policy` impl in the
wrapper (`OpenPolicy` in store.rs refuses ARM); `deviceInfo` is a 10-line vendor-lane stub; the TS
tests' `device.faults` assertions become a refusal counter; exact `commitSequence`/`storeId`
literals become relative assertions; `sinkUploads` is used by nothing. The existing Rust
`flat_harness` moves into the same crate so there is one Rust virtual flat device, not two, and the
two things currently called `Device` get distinct names. The dependency graph resolves for wasm32
today (pure Rust); the first PR of this step runs the wasm-pack build to prove it.

Consumers: the 161 TS device-flow tests across 8 files, the browser dev-harness (manual today) and
the Playwright smoke that drives it. Not a consumer for now: Swift. The four scripted `TransferLink`
actors (368 LOC) plus the shared flat-store-v4 vectors remain the phone-side truth. Trigger to
revisit: a client bug traced to divergence between the actors and the engine. If that day comes,
the Swift conformance suite is written generic over `TransferLink` so a later BLE run against a DK
is a second instantiation, not a rewrite.

**OBCMock** (2.7k LOC) is the simulator app's scripted UI device for XCUITest and the dev panel. It
must never model the wire protocol; protocol behaviour comes only from the engine.

### 2.4 Where the tests live

| Surface | unit | system |
|---|---|---|
| firmware crates | inline (pure machines, screens via `Ctx`, codecs) | one `tests/main.rs` per crate |
| obc-app | inline; the ~100 scenario-shaped tests that are really unit tests (34 screen tests, ~40 machine tests, pass.rs stage-order tests) move next to their modules | `tests/main.rs` from today's 22 files (one binary) |
| obc-host-core | inline | the ~150 relocated obc-app scenarios over `Device` (~100 as-is, ~50 via the seeds and `screen_stack`); the conformance corpus unchanged, plus one run under `BOARD_SUPPORT` (the board's capability set: no detour, no retention metadata); **the screen sweep as one Rust test** (317 frames from one process, one map load, same sha256 manifest; `ui-snapshots.sh` deleted); RideRecovery rendered from `with_recovered_ride` |
| obc-sim | inline | `dirty_parity` over `Device`; weather product → screen: event-pack service tree → `FixtureHttp` → wx-client → OBCW → device reader → App frame (obc-sim already depends on wx-client, obc-weather and obc-fixtures) |
| obcm-assemble | inline | `oracle.rs` gains one test: assembled map → flat-store card (obc-host-core, tempfile) → `StoreSource` → App boot → one frame hash + read counters. No new crate. |
| storage / link / dfu | crash matrix, break matrix, engine suite as today | one chained scenario: ride journal → power cut → remount → GET → GPX; one "media cut inside a PUT" |
| board crate | the four decision predicates (next-wake fold, watchdog-feed gate, GPS power, `owed`) extracted as pure functions into obc-app next to `residual.rs` and unit-tested there (~60–80 LOC moved, synchronous code, no stack change) | none: RideExec's phase order stays on the board with #1262's physical evidence; the host-side proof of the board's shape is the corpus under `BOARD_SUPPORT` |
| web | vitest unit + component; bundle guard reads `build:all` output instead of building four times (−21 s) | flow tests over the wasm engine; one Playwright spec over the dev-harness (Chrome channel on ubuntu-latest, no browser download) on the 72 KB fixture, seconds; a second, heavy spec for the #1503 memory measurement over large inputs |
| iOS | per-target `swift test --filter` | unchanged; XCUITest pruned to the ~17–20 methods the host cannot see, run at heavy cadence |
| desktop | rides.rs filesystem tests (Linux per PR) | `--smoke` launch under xvfb on the Linux leg (closes #994); macOS/Windows build-only on push |

### 2.5 Selection and CI

Replace the suite registry and selector with one ~300-line script:

- Rust: `git diff` → changed packages → reverse-dependency closure (the existing 50-line function
  over `cargo metadata`, kept) → `cargo nextest run -p a -p b …`. The `-p` list is what narrows the
  compile; a filterset narrows only what runs, so it is used just to carve heavy targets out.
  Manifest, lockfile or toolchain changes select everything.
- Feature shape, stated honestly: CI runs one workspace-unified invocation without `--all-features`
  (that flag only toggles `external-fixtures` and board-only features on the host build). Cargo
  still unifies `alloc`/`std`/`obcg-deflate` across the selected packages, so the 1 s
  `cargo test -p obc-formats` default-shape step stays, and local `obc test -p` keeps compiling the
  per-package (device) shape as it does today. The six `external-fixtures` features go; fixture-
  gated files become unconditional and fail with the sync command if the package is missing.
- Non-Rust surfaces are a dict of directory prefixes → job names inside the same script: web,
  ios-unit, ios-app (build + mock-seam grep only), desktop, board, boot, wasm-bridges (gated on web
  or desktop being selected), docs, tools.
- Jobs (about twenty, down from 26; half skip on a typical change): selection (alone, ~13 s),
  guards (the seven grep guards merged into one rule-table script + fixture policy + tooling tests,
  one parallel job), fmt, clippy, test (affected `-p` set: nextest, doctests, the obc-formats step,
  obc-bench golden in the debug profile), sweep (selected by reverse deps of app/render/sim),
  weather-captured (one leg: captured contracts + derecho rebake, ~60–90 s), embedded, boot, device,
  deny, wasm, wasm-bridges, web (vitest + svelte-check + build:all + bundle guard + rain-radar +
  builder FastAPI tests), desktop, ios-unit, ios-app, docs, aggregate.
- Aggregate: "every planned job succeeded, none skipped or cancelled", printing job / result /
  wall time from the `needs` context. ~40 lines. No JUnit parsing; the native JUnit uploads stay
  for the on-demand timing-diff script (nextest only).
- Heavy: `workflow_dispatch` plus a release-time run. No cron until someone is named to read it.
- Kept from today: fail-closed (an unowned test root or production path is an error), skipped-
  selected-is-a-failure, and the 15-row change-class table as the script's only test.
- Cache: the develop-push run stays a full build so the shared cache stays complete; split-out jobs
  each pay ~30 s of restore + host deps plus the workspace-member compile rust-cache never stores,
  so wall time drops while runner-minutes rise slightly. Acceptable, stated.
- Deleted: the three ci.yml regex walkers, `validate-filters`, `gates`, `explain`, the level
  selectors, all exception bookkeeping and the weekly issue-health workflow, coverage-policy.toml,
  the two zero-test suites, the `fixtures` field. Estimate −3,300 LOC across selector, registry,
  aggregate and tooling tests.

Local vocabulary: `obc test -p <crate> [filter]` (unchanged, now nextest), `obc test affected`
(the same `-p` set CI runs), `obc test heavy <name>`, `obc test full` (every job script), `obc
fixtures sync`. `obc check <gate>` goes away once each CI job is a script.

### 2.6 The glance view

The CI run page is the map: about twenty jobs named by surface, and the aggregate's table of job /
result / wall time. `obc test affected` prints the plan it will run with one reason per package
(reverse dependency of X, path prefix Y). No dashboard, no trend store; the on-demand timing diff
over the last N runs' nextest JUnit files is a 50-line script.

### 2.7 Build-time levers, ranked by measured payoff

| Lever | Effect | Risk / notes |
|---|---|---|
| One `tests/main.rs` per crate (159 → ~75 binaries; obc-app 23 → 2) | build 12.7 → ~4–5 s (measured bound 2.9 s for lib + one test), first-launch 44 → ~4 s, clippy 10.7 → ~4–5 s (estimate) | low. Exceptions: obcm-assemble stays at 2 (a global allocator test); obc-wx-bake splits by weight into named targets (keeps #1660's split); `overlay_plane.rs` swaps the global panic hook, so the local runner must be nextest (process per test); ~150 `use common::` paths change |
| Cut heavy dev-deps | obc-dem: move `COPERNICUS_ATTRIBUTION` to obc-elevation (a feature flag would only help the single-package build). obc-route: either stop using the packer's writer in its tests (obcm-testkit already hand-writes OBCM) or split ~5.8k LOC (nav, serialize, poi, hours, grid) out of obc-pack into a GEOS-free `obcm-write` crate; the split frees obc-route only, obcm-assemble's oracle needs the whole packer by design | low / medium (module move) |
| Move sweep, bench golden, pytest out of the `test` job | `test` 233 → ~146 s; the sweep runs only for rendering changes (your rule) | low |
| One `guards` job instead of nine spin-ups; selection stays alone | ~80 runner-s per PR | low |
| iOS: cache SwiftPM/DerivedData; take the 6–9 min screenshot-drift check off the PR gate (assets regenerate by hand when the landing page changes); prove the mock seam with a package-level release build | ios-app 13–18 → ~4–6 min | low |
| Desktop: Linux-only per PR with the launch smoke; macOS/Windows on push | ~6 CPU-min per selected run | low |
| Web: bundle guard reads `build:all` output | vitest 63 → ~42 s CPU | low. The sha256 seam and the flow-payload shrink stay rejected (#1502); not levers |
| Weather: fast half (unit + synthetic + opera + publication, ~10 s) inside `test`; captured contracts + derecho rebake in one leg; airmass, truth ladders and fuzz loops at heavy cadence | no 4-leg matrix | your cadence call on airmass (#1496) |
| Fixtures: drop sim-peak-view (103 MB, read by no test) from the `test` profile | CI cache and first sync shrink from 142 MB to 43 MB | none |

Not levers (measured): `CARGO_INCREMENTAL=0`, `debug=0`, combined `-p` invocations, sccache, mold or
lld (unavailable on macOS; ld_prime is already the linker).

### 2.8 Deletions (per-item yes needed)

| What | LOC | Safety |
|---|---:|---|
| Retired S0 BLE transfer protocol: `obc-ble` transfer.rs, list.rs, the Transfer* half of descriptor.rs and the msg 1/2/4 arms of `StatusMessage`, their tests (dfu.rs keeps its three `install_fw` tests), the obc-vectors builders, 8 vectors, spec §4.3 | ~2,000 | no user outside obc-ble (grep incl. the board and iOS); `TransferControl` is embedded in the live `StatusMessage` envelope, so the notify buffer shrinks and the resource baseline is re-measured in the same PR |
| `tools/s6b_board_cutover_soak.py` + its test | 1,233 | past its death trigger; git keeps the Link code; the rig laws are in memory |
| `tools/bench_ingest.py` + its test, and `flat_store_bench`'s serial-ingest mode | 1,079 | **owner-gated**: no death trigger; the board README still names it as the way to load a map before the bench; needs confirmation that the desktop USB v4 upload works on the bench rig, and a README edit |
| `loc_ledger.py`, `loc_report.py`, their tests, two justfile recipes | 1,657 | per-merge LOC bookkeeping is dead (your 2026-08-28 ruling). `dev_cleanup.py` and `docs_copy.py` are `obc clean`/`obc docs` and stay |
| ~57–60 XCUITest methods with host-model twins; 14 launch helpers → 1 | ~1,600 | never ran; **precondition**: a 77-row method → twin table in the deletion PR |
| `nowcast_skill_events.rs` (runs per PR, 36 s, one product assertion) and `nowcast_cost.rs` (#[ignore]) | 1,132 | keep the crossover assertion in nowcast_skill.rs |
| `OBCMockTests` pacing/knob/JSON tests | ~620 | keep the launch-arg parser, scenario table and default-fixtures pins (~340) |
| board bin `ble_central_repro` (#707 closed) | 347 | `display_test` (181) is a panel-wiring bisect tool: **your call**; `flat_store_bench` stays (CI frame-ceiling input) |
| `feeders.rs` | 326 | doc-as-code with no importer; the trace-based feeder gate is independent and stays |
| legacy-library compat pins (Swift) + 2 JSON fixtures | ~190 | pre-release rule; any v1 read path in product code is a separate product change |
| ci.yml history comments (~313 lines by explicit ranges), resource_baseline.json's 80 `_*_note_*` keys (165 KB of 171 KB; never read by resource_guard), obcm-testkit revision history | ~340 + 165 KB | your comment rule; git is the archive |

Kept after review: `obc-app/tests/dirty.rs` (242 lines; eight of its assertions are "no repaint
happened", which the pixel-parity oracle cannot prove).

Totals: deletions 10.4–11.0k (of which ~2.4k owner-gated); consolidations 2.5–5.0k (executors,
fakes, helpers, registry); additions 4.5–6.5k (`Device` ~500, wasm glue ~220, the 317-frame sweep
table 400–700, job scripts 300–600, guards rule table ~200, selection script ~300, new system tests
~2.5k). **Net −6k to −11k, central about −9k, i.e. 5–6% of the 165k base.**

### 2.9 Goldens

One convention for every committed golden (screen sha256 manifest, obc-bench golden.txt,
web-assemble expected maps, vector fixtures): the same test regenerates under
`OBC_UPDATE_GOLDENS=1`; no separate `update` subcommands, no `#[ignore]` regenerator tests.
Recommendation: keep all 317 screen digests. Once the sweep is one in-process test, an update is one
command and a reviewed diff of the frames that changed; the churn was a cost of the bash pipeline, not
of the digests. The vectors get one producer (`obc-vectors regen`) that also writes `manifest.json`,
which is hand-maintained today and trusted by Swift and TS as ground truth; this lands last.

## 3. What this overturns (explicit)

| Decision in force | This plan |
|---|---|
| L1 seven-level taxonomy (#1449) | two kinds × three cadences; contract and fixture stop being levels; live leaves the test system |
| L2 per-suite selection; split, never relabel | per-package selection via cargo; "split" means one PR target plus named heavy targets |
| L3 one registry of non-derivable facts; L4 one selection engine | still one engine, ~300 lines with a prefix dict; no TOML registry |
| L6 fixed local interface incl. `obc test unit\|component\|contract\|e2e` | `-p`, `affected`, `heavy`, `full`; `obc check <gate>` (L22) folds into job scripts |
| L8 coverage ratchet for the safety core | deferred: one on-demand `cargo llvm-cov` recipe, no policy file until a first measurement exists (touches #1448 Gate 5 wording) |
| L11 every exception links an open issue; `obc suites check-issues`; the weekly health workflow (merged 2026-09-12) | deleted: no exception class survives the tier collapse; a slow test is either heavy or gone |
| L19 time suites in the `--workspace --all-features` shape (#1565) | the timing basis is the workspace-unified shape without `--all-features`, nextest ci profile |
| L13 no production change to speed a test | kept, and every production change this plan makes is listed here so you can refuse each: (a) two typed seeds and a `screen_stack` readout on `App`, because scenario tests are a consumer of `App`; (b) `COPERNICUS_ATTRIBUTION` moves to obc-elevation; (c) obc-route's tests stop using the packer's writer, or obc-pack is split so obc-route's dependency graph matches the device's; (d) the six `external-fixtures` Cargo features are deleted; (e) four board decision predicates move into obc-app; (f) obc-sim's headless assembly becomes a library. Not made: HostLoop `execute` split; sha256 seam; obc-sim GUI re-hosting |
| #1262 (2026-09-12) "no further extraction by default" | respected: only the four synchronous predicates move (its own amendment allows "pure decision logic to a host-testable crate"); the phase order stays on the board |
| #1495 "no new test-support crates" | that rule was scoped to TS4; the bar here is your scaffolding rule: each harness has named consumers and no death trigger |

Rejected proposals R1–R19 are not re-proposed. The first draft carried the sha256 length seam and the
flow-payload shrink; both were struck.

## 4. Sequencing

1. **Speed first** (mechanical, measured before/after, 2–3 PRs): `tests/main.rs` per crate; local
   runner → nextest; dev-dep cuts; drop `--all-features` and the `external-fixtures` features; sweep,
   bench, pytest out of `test`; one `guards` job; iOS cache and screenshot check off the gate;
   desktop Linux-only per PR; sim-peak-view out of the test profile. Two five-minute experiments
   ride along: time `cargo test -p obc-app --lib --no-run` with and without an obc-host-core dev-dep
   (decides where the scenarios live), and your own first-launch measurement.
2. **CI honours the plan, minimal cut**: feed the `-p` set into `test`; delete the ~770 lines of
   walker-consistency proofs, coverage scaffold and zero-user vocabulary; aggregate prints the table.
   (~1,500 LOC gone; L1–L6 nominally still in force.)
3. **`Device` and the relocation**: export the headless assembly; retarget `support.rs`, Sweeper,
   dirty_parity and obc-sim; move ~150 scenarios into obc-host-core's tests and ~100 unit-shaped
   ones next to their modules; delete the `cfg(test)` plumbing; the sweep becomes one Rust test.
4. **`obc-flat-device`** (wasm): the TS flow tests and the dev-harness on firmware truth; MockDevice
   deleted; Playwright smoke over the dev-harness.
5. **Registry → script, fill the top, delete the dead**: the job table rewrite; map round trip in
   the assembler oracle; weather product → screen in obc-sim; chained storage/ride scenario; board
   predicates; desktop launch smoke; the deletion list; goldens convention; vectors generator last.

Each step lands on green CI with one review round. Steps 1–2 are independent of 3–5.

## 5. Decisions I need from you

Architecture
1. Scenarios live in obc-host-core's tests (no dependency cycle; `obc test -p obc-app` = unit only) — recommended — or in obc-app's tests (cycle; measure first)?
2. Conformance corpus keeps CoreHarness as its policy-free executor — recommended.
3. Protocol device: wasm only, Swift stays on its scripted links with a named trigger — recommended.
4. The six production changes in the L13 row: yes/no per item; for (c), tests-off-the-packer (small) or the obcm-write split (~5.8k LOC moved)?

Infrastructure
5. Overturn the #1449 registry/taxonomy decisions as in §3, in two steps (minimal cut first) — recommended — or keep the registry and only make CI honour its plan?
6. Local runner becomes nextest (one `cargo install`, already in `obc doctor`) — recommended.
7. Fixtures: explicit `obc fixtures sync`, tests fail loudly with the command — recommended — or auto-sync on first run?
8. Heavy cadence: `workflow_dispatch` + release time, no cron — recommended.
9. Targets to hold the work to: app-crate unit loop ≤ 10 s, one-crate CI ≤ 3 min wall, no PR job over 6 min.

Content
10. The deletion list, per row; the owner-gated rows (bench_ingest, display_test, XCUITest method table).
11. Goldens: keep all 317 digests under one update command — recommended — or a curated set?
12. Airmass rebake: heavy cadence (derecho stays in the PR leg) — recommended.
13. Coverage: defer the ratchet (recommended) or keep TS5 for #1448 Gate 5?
14. Playwright as a dependency for one PR spec + one heavy spec — yes/no?
15. LOC convention for the epic: brace-matched `#[cfg(test)]` blocks — recommended.

Hardware
16. This epic delivers the seam only: the s6b Link code is archived in git, #1177 is rewritten against the current firmware landmarks (`flat: catalog holds … map object(s)`, `flat: map object … open` after reboot, no `MAP UNREADABLE`, no `SD: misaligned block buffer`, `store-census` before/after), and `obc smoke-upload` (~600–700 LOC of Rust: VCOM link ~200 + landmark asserts + a native USB PUT sender that does not exist yet) is built when #1393 or #1262 needs physical evidence — recommended. Or build it now?
17. Please run `time <fresh test binary> --list` twice in your own terminal after any Rust test build to confirm the first-launch cost.
