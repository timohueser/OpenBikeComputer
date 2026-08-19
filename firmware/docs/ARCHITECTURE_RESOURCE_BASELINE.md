# Firmware architecture and resource baseline

The machine-readable authority is
[`tools/resource_baseline.json`](../tools/resource_baseline.json). CI enforces it with
[`tools/resource_guard.py`](../tools/resource_guard.py). This document explains how to reproduce
and interpret that contract; historical per-change accounting belongs in Git history.

The board target is `thumbv8m.main-none-eabihf`. The JSON records the exact source, Rust/LLVM and
RISC-V toolchain used for each accepted re-pin. CI's `embedded` job is the authority for absolute
shipping-image figures because different hosts can make different inlining and flash-placement
choices.

## Reproduce the automated baseline

Install the target plus the Rust and RISC-V tools named by the JSON. From `firmware/`, run the
host-side correctness and parser checks:

```sh
cargo test --workspace --all-features --locked
cargo run -p obc-bench --release --locked -- --check obc-bench/hashes.txt
python3 tools/check_dependencies.py
python3 -m unittest discover -s tools/tests -v
python3 tools/resource_guard.py strict-align
```

From `firmware/obc-fw-nrf54l/`, build and inspect each production profile before creating its
diagnostic report image:

```sh
cargo build --release --locked
python3 ../tools/resource_guard.py board --profile default \
  --elf target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l
cargo build --release --locked --features resource-report
python3 ../tools/resource_guard.py report --profile default \
  --elf target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l

cargo build --release --locked --no-default-features --features ble
python3 ../tools/resource_guard.py board --profile ble \
  --elf target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l
cargo build --release --locked --no-default-features --features ble,resource-report
python3 ../tools/resource_guard.py report --profile ble \
  --elf target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l
```

`resource-report` adds a `.obc_resources` table and therefore changes the ELF. Never flash or
package that diagnostic image; rebuild the normal profile first.

From `firmware/obc-boot/`:

```sh
cargo build --release --locked
python3 ../tools/resource_guard.py boot \
  --elf target/thumbv8m.main-none-eabihf/release/obc-boot
```

## Current gates

The numbers below summarize the checked-in JSON. Read the JSON for the current feature string,
toolchain and named-allocation table rather than updating a second copy here.

| Profile | Linked resident | `.uninit` ceiling | Flash record | Poll frame | Main task | Residual stack | Boot-chain ceiling | Deep-ride high-water |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| default | 321,784 B / 321,864 B max | 132,096 B | 1,570,976 B | 9,728 / 12,288 B | 7,264 / 8,192 B | 37,640 B min | 24,576 B | 35,808 B (+1,832) |
| BLE | 321,784 B / 321,864 B max | 132,096 B | 1,570,960 B | 9,728 / 12,288 B | 7,264 / 8,192 B | 37,640 B min | 24,576 B | 37,760 B (**−120**) |
| bootloader | — | — | 16,708 / 32,768 B max | — | — | — | — | — |

> ⚠️ **The BLE profile's board gate is RED on purpose.** Since FS7.5-c1 put the flat store in the
> image the residual stack is 37,640 B, against a deep-ride high-water last measured at ~35,808 B
> (default) / 37,760 B (BLE) on 2026-07-04 — so BLE is 120 B under water and default clears by
> 1,832 B. Schema v3's `deep_ride_high_water` / `deep_ride_margin_min` are what say so; before them
> nothing compared the residual to a *run*, only to its own approved floor, which is how those
> 11,848 B walked under a recorded peak with every check green.
>
> The high-water is stale — 2026-07-04, nothing has re-run it — but stale in an **unknown**
> direction, so it is carried unchanged until someone re-measures it on glass (the "on-device
> capture" section below). A measurement is replaced by another measurement, not by an argument.
> Read `board.default._resident_note_fs75c1` in the JSON before changing anything resident.

“Linked resident” is `.bss + .data`; `.uninit` is resident RAM too and carries the scratch arena.
Review the two together. `residual_stack` is measured from the end of all resident allocations, so
moving bytes between sections cannot pretend to save RAM.

Flash is recorded for the board profiles but not gated because host code generation varies. The
bootloader's flash budget is gated because its slot has a hard architectural size.

## Load-bearing invariants

- Exactly one device framebuffer and one scratch arena may be full-frame-sized writable objects.
- The arena costs the maximum of its render, navigation and USB arms, not their sum. Growth below
  the current maximum is free; growth of the largest arm costs one resident byte per byte.
- The report table names large allocations so a struct or buffer cannot grow invisibly inside a
  linked total.
- `poll_frame_limit` catches large async futures. `task_frame_limit` separately catches the main
  Embassy task body, which does not necessarily appear under the ordinary poll-symbol pattern.
- Boot-chain analysis is a conservative call-graph drift detector, not a stack-safety proof. The
  residual-stack and deep-ride high-water checks remain independent gates.
- `residual_stack_min` is self-referential — it is whatever the last approved build measured — so it
  catches drift, never insufficiency. `deep_ride_high_water` is the one gate that compares the
  residual against a number measured **on the board**, and it is the only one that can say a build
  has too little stack rather than merely less than before. It moves only with a fresh on-glass run.
- The `RESIDENT_BYTES` + `STACK_RESERVE` assert in `main.rs` is a coarse compile-time tripwire over
  hand-itemized blocks, **not** a stack gate: it undercounts the linked total by ~52.8 KB, because
  task pools, merged globals and padding are link-time facts a `const` cannot see.
- Strict alignment is enabled in both shipping Cargo roots and probed against the selected ARM
  backend.
- `firmware/tools/check_dependencies.py` enforces the device dependency direction. Heavy host
  policy and native libraries must not become device dependencies.

When a resource change is intentional, update the machine-readable measurement and explain the
architectural reason in the commit or PR. Do not append another permanent change diary here.

## Render and timing evidence

`obc-bench/hashes.txt` is the deterministic pixel authority. A pure refactor leaves it unchanged;
an intentional render change updates it with the visual evidence in the same PR.

```sh
cargo run -p obc-bench --release --locked -- --check obc-bench/hashes.txt
cargo run -p obc-bench --release --locked -- --repeat 9
```

Timing samples are comparable only with the recorded host, build profile and scene. They are drift
signals rather than cross-machine performance promises.

## On-device capture

Automated ELF checks cannot prove glass behavior or true stack high-water. For a change that affects
resource use, scheduling, presentation or hardware transport, record:

1. commit, compiler, feature set, board/panel revisions, power setup and map/config hashes;
2. at least 30 render samples for riding, rotated riding and dense overview scenes;
3. full-frame and representative dirty-row present times;
4. input/overlay latency both idle and while a dense render is running;
5. BLE and USB transfer contention where the changed path can overlap them;
6. default and BLE stack high-water after Home → route → ride → finish/save.

Use the physical button edge and a photodiode or equivalent display observation for latency claims;
USB command timing is not the authority. Store the resulting evidence with the change that needs it,
not as an indefinitely pending table in this baseline.
