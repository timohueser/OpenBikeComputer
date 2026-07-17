# Firmware architecture and resource baseline

This is the reproducible FAR-00 baseline for the firmware refactor epic (#792).
It records facts that later refactors must preserve or intentionally re-baseline:
linked memory, named compile-time allocations, rendering output/performance,
dependency direction, and observable on-glass behavior. The machine-readable
authority is [`tools/resource_baseline.json`](tools/resource_baseline.json); CI
enforces it through [`tools/resource_guard.py`](tools/resource_guard.py).

The numbers below were captured from source commit
`fded8d6444bd3e17fae7f916fd10a7eea0611700` with rustc 1.96.0
(`ac68faa20 2026-05-25`), LLVM 22.1.2, and target
`thumbv8m.main-none-eabihf`. The board crate currently selects the constrained
`nrf-mem` capacities for the nRF54L15 DK. The shared crates also have larger
LM20/simulator capacities, but there is no LM20 board ELF yet, so this document
does not invent an LM20 linked baseline.

The board ELF was built on an arm64 MacBookPro18,3 running macOS 26.5.1
(25F80), with `riscv64-elf-gcc (GCC) 16.1.0` and GNU objcopy/binutils 2.46.1
producing the FLPR blob. Record a new compiler/host whenever the ELF baseline is
deliberately refreshed.

## Reproduce the automated baseline

The repository intentionally tracks floating `stable` in `rust-toolchain.toml`.
For exact reproduction of these numbers, explicitly select the captured 1.96.0
toolchain rather than relying on the checkout default:

```sh
rustup toolchain install 1.96.0 --profile minimal \
  --component rustfmt,clippy,llvm-tools \
  --target thumbv8m.main-none-eabihf
export RUSTUP_TOOLCHAIN=1.96.0
rustc --version --verbose
```

Also install RISC-V GCC/binutils compatible with the recorded GCC 16.1.0 and
binutils 2.46.1; they build the board crate's FLPR blob. Then, from `firmware/`:

```sh
# Host correctness, deterministic pixels, and repeatable timing sample.
cargo test --workspace --all-features --locked
cargo run -p obc-bench --release --locked -- --check obc-bench/hashes.txt
cargo run -p obc-bench --release --locked -- --repeat 9

# Architecture direction and guard-parser regression tests.
python3 tools/check_dependencies.py
python3 -m unittest discover -s tools/tests -v

# Prove both shipping Cargo roots wire strict-align and the selected ARM backend honors it.
python3 tools/resource_guard.py strict-align
```

Build and inspect the two board profiles from `firmware/obc-fw-nrf54l/`:

```sh
# Default shipping image: inspect this ELF before creating the report ELF.
cargo build --release --locked
python3 ../tools/resource_guard.py board --profile default \
  --elf target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l
cargo build --release --locked --features resource-report
python3 ../tools/resource_guard.py report --profile default \
  --elf target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l

# BLE shipping image, then its separate report-only diagnostic image.
cargo build --release --locked --no-default-features --features ble
python3 ../tools/resource_guard.py board --profile ble \
  --elf target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l
cargo build --release --locked --no-default-features --features ble,resource-report
python3 ../tools/resource_guard.py report --profile ble \
  --elf target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l
```

`resource-report` emits a target-side `size_of` table in `.obc_resources` only
for inspection. It deliberately changes the diagnostic ELF. Rebuild the normal
profile after running it and never flash or package the report ELF.

From `firmware/obc-boot/`, verify the boot slot independently:

```sh
cargo build --release --locked
python3 ../tools/resource_guard.py boot \
  --elf target/thumbv8m.main-none-eabihf/release/obc-boot
```

## Linked ELF baseline

| Profile | `.bss` | `.data` | Linked resident | `.uninit` | Flash sections | Writable full frames | Largest guarded poll frame |
| :-- | --: | --: | --: | --: | --: | --: | --: |
| default | 202,912 B | 72 B | 202,984 B | 1,024 B | 670,128 B | 1 × 76,800 B | 52 B |
| BLE | 210,472 B | 4,504 B | 214,976 B | 1,024 B | 1,109,956 B | 1 × 76,800 B | 6,240 B |
| bootloader | — | — | — | — | 16,012 / 32,768 B | — | — |

Auto-delete hardening (#876, finding 2) gives retention a **full compact ride
inventory** in `CatalogState` — `heapless::Vec<RideRetentionRecord, MAX_RIDES>`,
one 8 B record (`id: u16` + `synced: bool` + pad + `synced_at: u32`, align 4) per
slot: 128 × 8 B + 4 B length = 1,028 B, landing as **+1,040 B** inside `App`
(35,992 → 37,032 B) with layout padding — so the expiry sweep reaches every
stored ride, not just the newest-32 UI catalog. The record cannot pack below
8 B without `repr(packed)`/split-array contortions that would save at most
256 B. The BLE profile additionally pays **72 B** for the two bounded
lossless delete `Channel`s (8 × `u16` slots each) that replace the overwriting
route/ride delete `Signal`s (finding 3). On pinned rustc 1.96.0, default grows
**201,944 → 202,984 B** resident and BLE **213,864 → 214,976 B**, the new
approved ceilings. The floating-stable rustc 1.97.1 (`8bab26f4f 2026-07-14`) CI
run links default at 202,976 B (preserving the repository's 8 B default
toolchain margin) and BLE at 214,976 B (exactly at its ceiling), with 658,484 B
/ 1,084,052 B flash; the pinned observations in the table are 670,128 B and
1,109,956 B. Framebuffer count, `.uninit`, and the guarded poll frames
(52 B / 6,240 B) are unchanged; no report entry other than `App` moves — the
delete channels are module statics, not `ObjectStore` fields.

Pure skip-ahead navigation (epic #789, RM3 #788) stores a forward-only route
floor and one route-identity-stable pending skip in `App`. This grows the named
`App` allocation **35,968 → 35,992 B** and linked `.bss` by the same **24 B**
on both profiles; no other named allocation changes. On pinned rustc 1.96.0,
default is **201,944 B** resident and BLE is **213,864 B**, which become the
approved ceilings. The corresponding pinned flash observations are 663,892 B
and 1,102,856 B. The `floating_stable_observation` entries in the machine-readable
baseline remain the last pre-RM3 rustc 1.97.1 measurements; applying the measured
24 B data-only delta would preserve default's 8 B toolchain margin and put BLE
at its new ceiling. CI remains the independent floating-stable check.

The mid-ride compass (epic #789, RM1 #786) deliberately raises the screen-stack
capacity from 8 to 10 so the deepest ordinary path still leaves two slots for
host-pushed cards. A target `Screen` is 84 B, so those two slots grow the named
`App` allocation **35,800 → 35,968 B** and linked `.bss` by the same **168 B**
on both profiles; no other named allocation changes. On pinned rustc 1.96.0,
default grows **201,752 → 201,920 B** and BLE **213,672 → 213,840 B** resident.
The approved default ceiling is 201,920 B: the current floating-stable rustc
1.97.1 (`8bab26f4f 2026-07-14`) links at 201,912 B, preserving the repository's
8 B default toolchain margin. BLE links at 213,840 B on both toolchains, so its
ceiling is that exact measurement. The same CI run observed 644,096 B default
flash and 1,068,784 B BLE flash; the pinned 1.96.0 observations in the table are
653,084 B and 1,091,960 B. Framebuffer count, `.uninit`, guarded poll frames
(52 B / 6,240 B), and every report entry other than `App` are unchanged.

Auto-expiry (epic #638, S3 #643) added the retention runtime + sweep queue to
`App`, per-route retention metas to `CatalogState`, and a `synced_at` to each
resident `RideSummary`, growing `size_of::<App>()` 34,944 → 35,800 B and, with
it, the linked resident: **default 200,868 → 201,752 B** and (stacked on #642's
+8 B setClock crossing signal) **BLE 212,796 → 213,672 B** — both ceilings
re-baselined to those figures. The `ride_retention` byte also grew `Settings`, so
the BLE object store (which embeds it) went 13,044 → 13,048 B and the BLE total
32,102 → 32,106 B. Measured identically on the local rustc 1.96.0 and the CI
floating-stable rustc 1.97.0 for BLE (213,672 B); the default links 201,744 B on
1.97.0, 8 B under the 1.96.0 capture that sets the ceiling. Poll frames
(52 B / 6,240 B) and the allocation report are unchanged in shape.

The table is the reproducible rustc 1.96.0 capture plus the itemized, approved
changes above. The first floating-stable CI
run on rustc 1.97.0 (`2d8144b78 2026-07-07`) produced default `.bss` 200,772 B,
`.data` 96 B, `.uninit` 1,024 B, and 615,416 B of flash: a measured 4 B resident
increase and a 9,784 B flash decrease caused by the toolchain change alone. The
default resident ceiling was explicitly re-baselined to 200,868 B from that CI
ELF; the original 200,864 B capture remains recorded separately. BLE stayed
exactly at 212,768 B resident with the same 6,240 B poll frame on that CI run;
its flash decreased to 1,034,048 B and its exact allocation report still
matched, so its ceiling did not change.

FAR-15 (#808, the instance-owned `SensorHub` replacing the process-global
sensor mailboxes) grew resident RAM by a measured **+68 B default / +88 B
BLE** against its merge-base develop, both sides built on pinned rustc
1.96.0: default 200,780 → 200,848 B (`.bss` 200,776 B + `.data` 72 B), BLE
212,672 → 212,760 B (`.bss` 208,256 B + `.data` 4,504 B). Per-symbol: the
new 156 B `SENSOR_HUB` static replaces ~156 B of deleted per-stream signal
statics (net ≈ 0), and the real growth is the handles threaded into task
futures instead of tasks reaching globals — `__embassy_main`'s pool +16 B
on both profiles (the ride loop's consumer/control handles plus its
select-arm event-wait), `sensor_task`'s pool +8 B (its link argument), and
on BLE `ble::run`'s pool +32 B (the `SampleInjector` held across the awaits
of each nesting level, run → sensors::run → run_link → serve_link) — plus
~30 B of linker global-merge/alignment reshuffle from swapping twelve small
statics for one large one. Two accidental costs were offset in review: the
ride loop had parked one hub pointer per stateless `*Source` in the main
future for the loop's lifetime (now call-expression temporaries at the
synchronous `app.tick` sites, −24/−32 B), and the fix mailbox's
niche-encoded `State::None` tag had dragged the whole 156 B hub static into
`.data` (now stored niche-free, keeping the hub all-zero `.bss`; `.data`
lands below its develop size on both profiles). The remaining growth is
inherent to instance ownership and was accepted by the owner (2026-07-14:
a handful of bytes is fine, only hundreds would matter); it fits under the
existing ceilings above, which are unchanged.

FAR-19 (#812, deleting the `take_*`/`notify_*` host-protocol compatibility
adapters and moving the board/sim/host-core onto the typed
`drain_host_commands`/`apply_event` protocol) grew resident RAM by a measured
**+24 B on both profiles** against develop, both sides on pinned rustc 1.96.0:
default 200,840 → 200,864 B, BLE 212,760 → 212,784 B. The growth is the board's
per-pass `HostPass` staging struct: the pass now drains the whole typed protocol
once, up front, into a small board-local struct whose fields cross the pass's
awaits (the bulge push, the DFU install, the store lock) to each command's
original consumption site in the store phase — where the deleted adapters used
to drain each one inline as a store-phase-local temporary that never entered the
task future. The irreducible core is the three `Option<u16>` object-delete ids
plus the `Option<u16>` settings-persist revision (16 B) that genuinely must
survive from the drain to the store phase; the small `bool`/`Copy`-enum fields
pack into their padding. The 44 B `NavRequest` is deliberately **not** staged —
the planner's `.bss` slot is written from it synchronously at the drain
(`nav_begin` needs no store lock), so only a 1-byte flag rides into the store
phase; staging the full request measured +52 B default, the flag-only design
+24 B. Poll frames and the allocation report are unchanged (the mailbox is a
stack temporary, dropped before the first await). The default result fits under
the unchanged 200,868 B ceiling with 4 B to spare; the **BLE resident ceiling
was bumped 212,768 → 212,788 B** (the 212,784 B measurement plus the repo's 4 B
floating-toolchain headroom). The owner accepted the growth (2026-07-14: a
handful of bytes is fine, only hundreds would matter).

FAR-19 closeout (#812, the final audit — removing the format/codec/hours/POI
compatibility aliases, privatizing the `Activity` staging fields behind an
in-crate test harness, deleting the dead `take_update_confirmed` accessor, and
the CRC/unsafe/panic/host-mutation audits) is **resident-RAM-neutral: +0 B on
both profiles**. It repoints imports to `obc-formats`, changes field visibility,
and deletes re-exports — it introduces, moves, and removes no runtime data. The
board ELFs are `.bss`/`.data` byte-identical to the merge-base develop
(`5eccd2ee`, which already carries FAR-19 parts 1–2), both built on pinned rustc
1.96.0: default `.bss 200,800 + .data 72 = 200,872 B`, BLE `.bss 208,280 +
.data 4,504 = 212,784 B`; poll frames (52 B / 6,240 B) and the allocation report
are unchanged. Render hashes are unchanged (bench 7/7) and the nine-run host
timing medians are within the reference host's recorded noise. No ceiling moved:
the default `resident_ram_max` stays 200,868 B and BLE stays the FAR-19 part-2
212,788 B. (Note the local-vs-CI trap: on this rustc 1.96.0 host **develop
itself** links 200,872 B default — above the 200,868 B ceiling that was
re-baselined from the 1.97.0 floating-stable CI ELF — so the local overage is a
toolchain artifact shared by develop, not drift introduced here; the CI contract
is the authority and this closeout's delta against it is zero.) Flash grew a
small amount from making `obc-formats` a direct `obc-app`/board dependency (the
"consumers import the authority directly" outcome): default +1,720 B, BLE
+1,592 B against develop on 1.96.0 — untracked headroom on the 512 KB-flash
target, no gated budget affected.

Auto-expiry S4 (#644, the `setRouteRetention` command + the `routeList` entry's
84-byte expiry tail) is **resident-RAM-neutral: +0 B on both profiles** and
**allocation-report-neutral**. The command decode/handler and the entry-tail
encode are code, not resident data, and the `routeList` entry buffer did **not**
grow — the shared list scratch (`LIST_BUF_LEN`) is sized by the *larger* list,
which is `rideList` (128 × 72 B = 9,216 B), still dominating `routeList`'s
64 × 84 B = 5,376 B. Built on rustc 1.96.0 both board ELFs are `.bss`/`.data`
byte-identical to develop (default 201,752 B, BLE 213,672 B), the poll frames
(52 B / 6,240 B) and all 22 allocation-report entries unchanged (`ble_object_store`
stays 13,048 B — no field was added to any resident struct). Only flash grew —
default +704 B, BLE +1,448 B on 1.96.0 — untracked headroom on the 512 KB-flash
target, no gated budget affected. The `resident_ram_max` ceilings and the numbers
table above are therefore unchanged; CI's `embedded (build (ble, release))` guard
and report pass against the S3 baseline unmodified.

The final production dependency graph has **69 local edges, zero exceptions**,
all pointing downward (`python3 tools/check_dependencies.py`). Against the
FAR-00 baseline's 64, the epic's net change is: #807/#857 severed
`obc-storage → obc-route` and added `obc-storage → obc-formats` +
`obc-formats → obc-ports`; #812 added the direct `obc-formats` import edges from
every persistent-format consumer that previously reached the byte-I/O seam and
format constants through a reader/route re-export — `obc-app`, `obc-fw-nrf54l`,
`obc-host-core`, and `obc-sim`. The byte-I/O seam (`ByteSource`/`ByteSink`/
`SliceSource`/`Error`) is now imported from `obc_formats::io` at every call site;
`obc-route` re-exports only `Error`, solely so its own `track_to_gpx`/`ByteSink`
writer signatures can name it. No upward edge exists; CI rejects every removed one.

FAR-642 (#642, the BLE `setClock` command — the phone stamps the device's UTC
clock + offset on every connect, auto-expiry epic #638 S2) grew resident RAM by a
measured **+8 B on BLE / +0 B on default** against develop on pinned rustc 1.96.0.
The +8 B is the `BLE_CLOCK_SET` crossing signal — a `Signal<_, (u32, i16)>` the
command handler posts and the ride loop drains into `App::stamp_clock_ble` (the
same lock-free module-static hand-off the other BLE→ride-loop crossings use). BLE
`.bss` 208,280 → 208,288 B (`.data` unchanged 4,504 B), so resident 212,784 →
212,792 B; the **BLE resident ceiling was bumped 212,788 → 212,796 B** (the 1.96.0
measurement + the repo's 4 B floating-toolchain headroom, matching the FAR-19
part-2 convention). **Default is byte-identical** (`.bss 200,800 + .data 72 =
200,872 B`): the same static is linker-GC-stripped there because `post_ble_clock`
/`take_ble_clock` are only reachable under `--features ble`, so its ceiling stays
200,868 B and the local-1.96.0 200,872 B overage remains the documented
toolchain-artifact trap, not new drift. The compile-time allocation report is
unchanged on both profiles — the signal is a module static, not a field of `App`
or `ObjectStore`, so no named entry (`app`, `ble_object_store`, …) moved.

“Linked resident” is the CI contract's `.bss + .data`. `.uninit` is reported
separately. The M33 receives 253,952 B after the FLPR carve, leaving 51,008 B
after default `.bss + .data + .uninit` and 39,088 B after BLE. Those residuals
are linker headroom, not measured stack high-water. The independent 36,864 B
`STACK_RESERVE` is a compile-time floor, also not a runtime measurement.

The framebuffer guard requires exactly one `FB` symbol of 240 × 320 × 1 byte
and exactly one writable symbol at least that large. Both shipping profiles'
disassembly guards check every `TaskStorage<F>::poll` prologue and retain the
12,288 B safety ceiling; the current maxima are 52 B for default and 6,240 B
for BLE.

## Named compile-time allocations

These are exact 32-bit target `size_of` values from the report-only ELF:

| Allocation | Default | BLE |
| :-- | --: | --: |
| framebuffer | 76,800 B | 76,800 B |
| row diff | 1,284 B | 1,284 B |
| `App` | 35,968 B | 35,968 B |
| map cache | 14,444 B | 14,444 B |
| map tables | 4,060 B | 4,060 B |
| route cache | 6,180 B | 6,180 B |
| route index | 6,252 B | 6,252 B |
| renderer (embedded in `App`) | 8,352 B | 8,352 B |
| navigation scratch / tile cache / planner | 19,976 / 4,140 / 9,024 B | 19,976 / 4,140 / 9,024 B |
| stack-reserve floor | 36,864 B | 36,864 B |
| BLE total | 0 B | 32,106 B |
| └ SDC memory / host resources / packet pool | 0 B | 8,704 / 3,960 / 4,036 B |
| └ object store / server / GAP name / sensor manager | 0 B | 13,048 / 1,936 / 52 / 370 B |
| └ MPSL handle / Cracen handle | 0 B | 0 / 0 B (zero-sized handles) |

The BLE object store (and so the BLE total) shrank 4 B — 13,048 → 13,044,
32,106 → 32,102 — when `Settings::gps_time` was removed in #641 (the store embeds
a `Settings`; `App` was unaffected, as `clock_trust` reclaimed the freed byte),
then grew the same 4 B back — 13,044 → 13,048, 32,102 → 32,106 — when the
`ride_retention` setting was added in #643 (auto-expiry S3).

Do not add this table to predict RAM. `App` embeds the renderer, the navigation
types are described even where `has_nav` makes them non-resident, and the stack
reserve is a floor rather than an allocation. The production ELF's linked
sections are the resident-memory authority.

## Host rendering baseline

Reference host: MacBookPro18,3, Apple M1 Pro (6 performance + 2 efficiency
cores), 16 GB RAM, macOS 26.5.1 (25F80), rustc 1.96.0. A nine-matrix run was
taken after building release code; every matrix uses the benchmark's warmed
minimum of ten renders. Hashes were stable across all repetitions.

| Scene | Minimum | Median | Maximum | Observed max deviation from median | Golden hash |
| :-- | --: | --: | --: | --: | :-- |
| riding | 126 us | 127 us | 134 us | 7 us | `56d9a197e3d0cfe7` |
| riding, rotated | 163 us | 168 us | 179 us | 11 us | `8351ed68501a0af6` |
| mid | 101 us | 103 us | 107 us | 4 us | `3071f09bdcd8bbe5` |
| mid, rotated | 146 us | 151 us | 155 us | 5 us | `8b3421f22d2e8ef5` |
| overview | 2,895 us | 2,923 us | 2,949 us | 28 us | `07b3e659b578f89c` |
| overview, rotated | 2,894 us | 2,927 us | 2,967 us | 40 us | `cde40d6215c2b9c0` |
| route | 118 us | 120 us | 124 us | 4 us | `e26019c8e500f2c0` |

Timing is intentionally not gated on shared CI runners. Compare before/after
nine-run medians on the same idle host. The observed deviations above are the
reference host's noise allowance: a larger change is a reason to repeat and
investigate, not an automatic percentage-based failure. Pixel hashes remain the
deterministic CI gate.

## Dependency-direction baseline

`tools/dependency_rules.json` groups the production packages into foundation,
core, app, platform, and host layers. Production normal/build edges may not
point upward; development-only fixture edges are ignored. New packages fail
until classified exactly once, and disappeared exceptions fail as stale. The
current graph has 64 production local edges and zero exceptions:
`obc-platform -> obc-app` was removed by #797, and platform adapters now import
their semantic contracts directly from `obc-ports`. #807 split `obc-platform`
into four platform-layer crates (`obc-display`, `obc-sensors`, `obc-storage`,
and the narrowed `obc-platform`), adding the consumer edges to the new crates
(58 -> 64) without introducing any upward edge.

The checker covers the `firmware/Cargo.toml` workspace plus the standalone
manifests for the excluded `obc-fw-nrf54l` composition root and `obc-boot` boot
root. Their production edges therefore participate in the same layer graph even
though both crates remain independently built and linted in CI.

The strict-alignment guard first parses both excluded Cargo configs and requires
their embedded-target rustflags to select `+strict-align`. Its backend probe is
also behavior-based: with
`+strict-align`, an align-1 four-byte decoder must lower to four byte loads and
no word load, while the no-flag control must contain a word load. Rustc 1.96.0
prints an “unknown and unstable feature” diagnostic but its LLVM
backend honors the feature; CI checks emitted assembly so a future toolchain
change cannot silently alter that assumption.

CI intentionally uses the floating stable toolchain declared by the repository.
If stable changes linked sizes or code generation, the resource gate must fail;
the resulting drift needs an explicit, measured re-baseline rather than a silent
toolchain assumption. Its FLPR build likewise installs Ubuntu's distro-current
`gcc-riscv64-unknown-elf` package rather than pinning the local versions above:
CI validates the resource contract, but does not promise a byte-identical copy
of the captured macOS board ELF.

## On-device capture (required before merge)

Automated checks cannot replace an nRF54L15-DK + LS021/FLPR capture. Record the
commit, rustc, exact feature set, DK/panel revisions, power setup, ambient
conditions, map SHA-256 plus packer preset/config, and logging level. Use a
representative dense map and the shipping artifact being approved; a
`debug-uart` artifact may be used for command-driven camera setup, but must be
identified separately.

1. Warm each camera five frames, then capture at least 30 samples at riding
   (0.5 m/px, north-up), riding rotated (35°), and dense overview (30 m/px,
   rotated). Report minimum, median, and p95 for render time. Use existing RTT
   `map frame` telemetry to split render and push; `debug-uart`'s `T` line is
   render-only.
2. Capture a 320-row full present and a clock/overlay partial present. With
   `DEFMT_LOG=debug`, record the existing `LS021 FLPR` dirty-row count and push
   time. Do not add instrumentation to the measured path.
3. Put a photodiode/reflectance sensor over the right-edge hold bulge under
   controlled lighting and capture it with the active-low SELECT signal on an
   oscilloscope or logic analyzer (at least 5 kSa/s). Measure physical edge to
   first visible transition, transition-to-transition cadence, confirm pop,
   release/retract, and final trailing-edge clear.
4. Repeat step 3 while a dense overview render is already running: set 30 m/px,
   start the redraw, then begin the hold inside its measured render interval.
   Confirm with RTT timestamps that the input/overlay sequence overlapped the
   long render. An idle-only overlay test does not establish preemption.
5. Repeat the contention case on the BLE artifact while connected and actively
   transferring/scanning route or ride-store objects. Record whether overlay
   latency/cadence or present timing changes.
6. Run Home → route load → ride → finish/save, plus the BLE/store contention
   case, from a fresh stack-watermark state. Record the default and BLE stack
   high-water values; do not substitute linker residual or historical comments.

`Z 30` and the existing heading/course debug controls can make camera setup
repeatable. For an automated hold, `K e d` can be sent after a calibrated delay,
but USB timing is not the latency authority: the physical button edge and
photodiode trace are.

### Device results

| Measurement | Result |
| :-- | :-- |
| render: riding / riding rotated / dense overview | **PENDING — owner device capture** |
| full present (320 rows) / representative partial present | **PENDING — owner device capture** |
| hold edge → first visible, idle / during long render | **PENDING — owner device capture** |
| overlay cadence, idle / during long render | **PENDING — owner device capture** |
| confirm pop / retract / final trailing-edge clear | **PENDING — owner device capture** |
| BLE + object-store contention | **PENDING — owner device capture** |
| default / BLE stack high-water | **PENDING — owner device capture** |

The PR carrying this baseline is not device-verified and must not merge until
the owner replaces these `PENDING` cells with captured evidence or explicitly
records an approved follow-up disposition.
