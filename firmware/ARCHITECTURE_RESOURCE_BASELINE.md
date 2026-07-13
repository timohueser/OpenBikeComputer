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
| default | 200,768 B | 96 B | 200,864 B | 1,024 B | 625,200 B | 1 × 76,800 B | 52 B |
| BLE | 208,240 B | 4,528 B | 212,768 B | 1,024 B | 1,055,284 B | 1 × 76,800 B | 6,240 B |
| bootloader | — | — | — | — | 16,012 / 32,768 B | — | — |

The table is the reproducible rustc 1.96.0 capture. The first floating-stable CI
run on rustc 1.97.0 (`2d8144b78 2026-07-07`) produced default `.bss` 200,772 B,
`.data` 96 B, `.uninit` 1,024 B, and 615,416 B of flash: a measured 4 B resident
increase and a 9,784 B flash decrease caused by the toolchain change alone. The
default resident ceiling was explicitly re-baselined to 200,868 B from that CI
ELF; the original 200,864 B capture remains recorded separately. BLE stayed
exactly at 212,768 B resident with the same 6,240 B poll frame on that CI run;
its flash decreased to 1,034,048 B and its exact allocation report still
matched, so its ceiling did not change.

“Linked resident” is the CI contract's `.bss + .data`. `.uninit` is reported
separately. The M33 receives 253,952 B after the FLPR carve, leaving 52,064 B
after default `.bss + .data + .uninit` and 40,160 B after BLE. Those residuals
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
| `App` | 34,952 B | 34,952 B |
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
current graph has 58 production local edges and zero exceptions:
`obc-platform -> obc-app` was removed by #797, and platform adapters now import
their semantic contracts directly from `obc-ports`.

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
