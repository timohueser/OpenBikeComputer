# Firmware architecture and resource baseline

This is the reproducible FAR-00 baseline for the firmware refactor epic (#792).
It records facts that later refactors must preserve or intentionally re-baseline:
linked memory, named compile-time allocations, rendering output/performance,
dependency direction, and observable on-glass behavior. The machine-readable
authority is [`tools/resource_baseline.json`](../tools/resource_baseline.json); CI
enforces it through [`tools/resource_guard.py`](../tools/resource_guard.py).

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

Current, **as CI links it** (the `embedded` job is the gate, and since #927 the JSON's
`resident_ram_max` / `measured_flash` are taken from there rather than from the pinned
host — see the note in `resource_baseline.json`). Both shipping profiles link an
identical resident set:

| Profile | `.bss` | `.data` | Linked resident | `.uninit` | Flash sections | Writable full frames | Largest guarded poll frame |
| :-- | --: | --: | --: | --: | --: | --: | --: |
| default | 288,672 B | 5,112 B | 293,784 B | 118,432 B | 1,300,320 B | 2 (76,800 B FB + 117,408 B arena) | 9,728 B |
| BLE | 288,672 B | 5,112 B | 293,784 B | 118,432 B | 1,300,320 B | 2 (76,800 B FB + 117,408 B arena) | 9,728 B |
| bootloader | — | — | — | — | 16,012 / 32,768 B | — | — |

(Flash is **CI's** figure, re-pinned from #1146 P2's `embedded` run; the pinned local host
links ~32 KB more for the same source, which is exactly why this column is not host-pinned.
The RAM figures include #1150's review round: +8 B of `App`, itemized in the JSON's
`_resident_note_1150_review`, and #1146 P3's +25,088 B of `.uninit` — the arena's render arm
growing into the frame caps, which is why `.bss + .data` is flat while `.uninit` is not.
Since #1146 the RAM story is `.bss + .data` **plus `.uninit`** — see the entries below.)

The rows below this line are the **historical** 256 KB-part figures kept for the
narrative they carry; they were never updated through the LM20 retarget and are an
order of magnitude below the table above. The machine-readable authority is, as ever,
[`tools/resource_baseline.json`](../tools/resource_baseline.json).

| Profile (historical, pre-LM20) | `.bss` | `.data` | Linked resident | `.uninit` | Flash sections | Writable full frames | Largest guarded poll frame |
| :-- | --: | --: | --: | --: | --: | --: | --: |
| default | 204,544 B | 72 B | 204,616 B | 1,024 B | 685,484 B | 1 × 76,800 B | 52 B |
| BLE | 211,032 B | 4,504 B | 215,536 B | 1,024 B | 1,124,104 B | 1 × 76,800 B | 6,240 B |

The USB device plane (#889) introduces `src/link/` — the transport-free companion-link
core (the §4.4 command handler, descriptor classification, the identity blobs, the
cross-transport one-transfer gate) and, with it, the single `ObjectStore`. The store
moved out of `ble::run` and is now built by `main` and handed to every plane, because
one SD card must have one catalog, one id allocator and one upload temp no matter how
many transports reach it. That lift is **+32 B resident on both shipping profiles**,
itemized per symbol on pinned rustc 1.96.0 (`llvm-nm -S` diff against the merge base):
`__embassy_main::POOL` **+24 B** (the `LinkStores` handle triple now crosses `main`'s
awaits from the store-epoch mint to the spawns) and `spawn_ble_stack::POOL` **+8 B**
(the same triple in the trampoline's argument pool, replacing two separate parameters).
Everything else nets to zero: the 13,528 B store static is byte-identical, renamed
`ble::STORE` → `link::STORE`, and the small `RECORDING` / `STACK_HIGH_WATER` /
`TRANSFER_ACTIVE` cells moved module without changing size. Both `resident_ram_max`
ceilings move **416,000 → 416,032 B** (the exact measurement — no toolchain margin was
added on top, matching the last several re-baselines); flash observations move to
1,146,736 B. Framebuffer count, `.uninit`, the guarded poll frames (9,664 B) and all 22
allocation-report entries are unchanged — `ble_object_store` still reports 13,528 B, now
sourced from `link::OBJECT_STORE_BYTES`.

**The USB device plane is now unconditional** (owner call, 2026-07-26): the `usb` Cargo
feature is gone, `embassy-usb` is a plain dependency, and `src/usb/` compiles in every
build. Both pinned profiles therefore *are* the USB shape — there is no non-USB shipping
ELF to compare against any more, and the two former `clippy (usb)` / `build (usb,
release)` CI legs were deleted because the default and BLE legs now build, lint and
resource-gate the same tree.

Re-measured on the pinned rustc 1.96.0 host against develop `7afc890d`:

| | before (no USB) | after (USB unconditional) | Δ |
| :-- | --: | --: | --: |
| `.bss` | 411,528 B | 415,992 B | +4,464 B |
| `.data` | 4,504 B | 5,136 B | +632 B |
| **linked resident** | **416,032 B** | **421,128 B** | **+5,096 B** |
| `.uninit` | 1,024 B | 1,024 B | 0 |
| flash (default / BLE) | 1,146,952 B | 1,192,640 / 1,192,632 B | +45,688 B |
| largest guarded poll frame | 9,664 B | 9,728 B | +64 B (limit 12,288 B, unchanged) |

Both `resident_ram_max` ceilings move **416,032 → 421,128 B** — the exact measurement,
no toolchain margin added, matching the last several re-baselines. Framebuffer count and
the two full-frame-sized writable symbols are unchanged. (The earlier `--features usb`
reference measurement in this document read 1,192,184 B of flash; the 448 B difference
is develop moving since, not USB — the *pre-change* default on this host measures
1,146,952 B against the 1,146,736 B recorded at its own pin.)

The resident cost is all named: `usb::run::POOL` 1,736 B (the task future — the three
joined planes plus their control frame buffers), the driver's EP-OUT staging buffer
1,088 B (EP0-OUT 64 + two 512-byte bulk OUT endpoints), embassy-nrf's `USBHS` endpoint
state 596 B, the control and bulk chunk scratch 512 B each, the MS OS 2.0 / BOS / config
descriptor buffers 256 + 96 + 96 B, the EP0 control buffer 64 B, and
`spawn_usb_stack::POOL` 64 B. The allocation report grows from 22 to **23 entries**: the
new `usb_named` entry reports `usb::RESIDENT_BYTES` = **2,644 B**, the crate-nameable
half of that list (the task future and embassy-nrf's endpoint bookkeeping are not
nameable from this crate and land only in the linked `.bss + .data` gate, which stays the
authority for resident RAM). It is itemized now precisely because it is unconditional:
growth in the newest resident block should be legible in the report rather than arriving
as a few thousand anonymous bytes.

**`floating_stable_observation` is refreshed here, and given provenance.** It had been
carried forward unchanged since the #876/#886 era — `resident 214,976 B`, `flash
1,084,052 B`, `poll_frame 6,240 B` — which after the LM20 retarget disagreed with the
gate sitting beside it in the same profile by 206 KB. Nothing regenerates the field, and
its `rustc` stamp (1.97.1, `8bab26f4f 2026-07-14`) happens to be *exactly* the toolchain
CI still runs, so the staleness was undetectable from the record itself. It now carries
this PR's CI measurement — `.bss 415,992 + .data 5,136 = 421,128 B` resident, `.uninit
1,024 B`, flash 1,166,584 B, poll frame 9,728 B, on rustc 1.97.1 / LLVM 22.1.6 /
`x86_64-unknown-linux-gnu` — plus `source_commit`, `source_run` and `host`, so a future
reader can date it instead of trusting it. Note what it says: the floating-stable
toolchain links the **same resident set and the same poll frame** as the pinned 1.96.0
measurement, to the byte; only flash differs (1,166,584 vs 1,192,640 B, and flash is not
gated on this target). Both board profiles link identically, so the single observation
under `ble` covers both.

**Stack margin.** `llvm-objdump -d --demangle` + the per-function `sub sp, #imm`
histogram puts the head of the shipping default at `link::init_store` 27,648 B,
`__embassy_main` 19,456 B, `ride::fill_ride_profile` 17,280 B, `NavPlanner::finish_emit`
16,064 B, `ride::nav_begin` 14,528 B — byte-identical to the pre-USB build. The deepest
USB frame is `usb::build_plane` at **956 B**, a transient boot frame (the
`embassy_usb::Builder`, kept out of the task's poll frame by `#[inline(never)]` — the
#677 rule); the deepest USB *async* frame is `usb::data_plane::run` at 108 B, and every
other USB-named frame is ≤ 44 B. So the plane changes the guarded poll frame by 64 B and
does not enter the histogram head at all. **On-glass stack high-water with the plane
enumerating is still unmeasured** — the last capture (56,292 / 69,448 B) predates USB, no
board was plugged in for this change, and a USB task adds ISR-context work a static
histogram does not model. See the pending device-capture table below; static frame
analysis is not a substitute for it.

**#936 — the VBUS gate, +64 B.** The first on-glass run of the unconditional plane did
not finish booting with J3 empty, which is the normal riding case; both profiles now
carry the fix's cost. Re-measured on the same pinned rustc 1.96.0 host, against the
develop merge base `333e1fca`:

| | before (#930) | after (VBUS gate) | Δ |
| :-- | --: | --: | --: |
| `.bss` | 415,992 B | 416,056 B | +64 B |
| `.data` | 5,136 B | 5,136 B | 0 |
| **linked resident** | **421,128 B** | **421,192 B** | **+64 B** |
| `.uninit` | 1,024 B | 1,024 B | 0 |
| flash (both profiles) | 1,192,640 B | 1,192,936 B | +288 B |
| largest guarded poll frame | 9,728 B | 9,728 B | 0 (limit 12,288 B, unchanged) |

Both `resident_ram_max` ceilings move **421,128 → 421,192 B**, the exact measurement with
no margin added, as every re-baseline before it. The 64 B is `usb::run::POOL` alone: the
task future gained the gate's poll loop — a `Timer` held across an await, the guard's
`poll_fn`, and the pinned endpoint join that used to be a bare `.await` temporary. **No
new static**, so `usb_named` is unchanged at 2,644 B and the report still has 23 entries;
`build_plane`'s 956 B transient boot frame and the 108 B `data_plane::run` async frame are
unchanged, which is the point — the gate adds a check before a poll, not a buffer. This
host measures the two profiles byte-identical for flash (1,192,936 B each); the recorded
1,192,632 B for `ble` predates it. `floating_stable_observation` is **not** re-measured
here — no CI run exists for this branch yet — and is flagged known-stale in the JSON with
the expected deltas, rather than left to look current.

**#937 — the event-driven VBUS park, +8 B resident and −80 B flash.** #936's gate parked
the plane on a 500 ms timer, so a device riding with nothing in J3 woke twice a second
forever to re-read a register only a human can change. The park now waits on a VREGUSB
interrupt — a board handler bound *alongside* embassy's on the same vector, waking an
`AtomicWaker`. A 30 s timer shipped alongside it as a self-healing net and was **removed
immediately after**, on glass evidence: `wait_for_vbus` registers before it reads, and reads a
level rather than a latched edge, so a wake cannot be lost and the net guarded nothing. That
removal is a further **−24 B resident / −496 B flash** (421,200 → 421,176; 1,192,864 → 1,192,368),
leaving the park a pure interrupt wait. Re-measured on the same pinned rustc 1.96.0 host, against
develop at `ccbf8100`:

| | before (#936) | after (event-driven) | Δ |
| :-- | --: | --: | --: |
| `.bss` | 416,056 B | 416,064 B | +8 B |
| `.data` | 5,136 B | 5,136 B | 0 |
| **linked resident** | **421,192 B** | **421,200 B** | **+8 B** |
| `.uninit` | 1,024 B | 1,024 B | 0 |
| flash (default) | 1,192,944 B | 1,192,864 B | **−80 B** |
| flash (ble) | 1,192,928 B | 1,192,856 B | **−72 B** |
| largest guarded poll frame | 9,728 B | 9,728 B | 0 (limit 12,288 B, unchanged) |

Both `resident_ram_max` ceilings move **421,192 → 421,200 B**. Unlike #936 this change
*does* add a static, and the whole +8 B is it: `usb::VBUS_WAKER`, verified at
`0x200656c8` by `llvm-nm`. It is counted into `usb::RESIDENT_BYTES`, so `usb_named` moves
**2,644 → 2,652 B** by exactly the same 8 and the report stays at 23 entries — the
resident step is a named term rather than an unexplained jump in the `.bss` gate. Flash
*falls*, because the timer loop cost more code than the `poll_fn` + `select` replacing it.
`build_plane`'s 956 B transient boot frame and the 108 B `data_plane::run` async frame are
unchanged.

Two measurement notes, because they contradict the paragraph above. This host does **not**
measure the two profiles byte-identical for flash: develop itself links 1,192,944
(`default`) vs 1,192,928 (`ble`) here, and 1,192,944 is already 8 B above the 1,192,936
recorded under #936 — so the *deltas* in this table are the trustworthy figures and the
absolutes are host-and-tree specific. `floating_stable_observation` is again **not**
re-measured (it predates both USB changes) and stays flagged known-stale in the JSON.

The routed detour (#882) replaces the pure skip: the `NavPlanner` gains the
resident **corridor blacklist** (`Option<Corridor>` — a 128-point downsampled
skipped-span polyline + inflated bbox + exemption coords), growing the
`nav_planner` report entry **9,024 → 10,096 B** (+1,072 B, in `.bss` on the
default profile only — the BLE image links no nav statics but shares the
table). `App` grows **37,040 → 37,592 B** (+552 B): the ≤64-point
detour-preview polyline slot in `CatalogState` (~520 B), the
`DetourRequest`/commit/cancel one-shots replacing the pure-skip slot on
`Activity`, and the `has_nav_graph` flag on `AppState`. On pinned rustc 1.96.0
default links **202,992 → 204,616 B** resident (+1,624 = both deltas exactly)
and BLE **214,984 → 215,536 B** (+552, `App` only), the new approved ceilings;
flash observations move to 685,484 B / 1,124,104 B (the detour planner, splicer,
and preview screens). Framebuffer count, `.uninit`, and the guarded poll frames
are unchanged. The `floating_stable_observation` entries remain the last
pre-detour rustc 1.97.1 measurements; CI's floating-stable run re-measures them
independently. (The board does not yet *dispatch* the detour commands — the
station stays dimmed on device until the ride loop feeds `set_map_nav_graph`
and drives the plan/commit pipeline — but the resident cost ships with the
shared crates, so it is itemized now.)

Auto-delete hardening (#876, finding 2) gives retention a **full compact ride
inventory** in `CatalogState` — `heapless::Vec<RideRetentionRecord, MAX_RIDES>`,
one 8 B record (`id: u16` + `synced: bool` + pad + `synced_at: u32`, align 4) per
slot: 128 × 8 B + 4 B length = 1,028 B, landing as **+1,040 B** inside `App`
(35,992 → 37,032 B) with layout padding — so the expiry sweep reaches every
stored ride, not just the newest-32 UI catalog. The record cannot pack below
8 B without `repr(packed)`/split-array contortions that would save at most
256 B. The #886 review round added the dispatched **id** to the two per-kind
in-flight delete slots (`Option<u32>` → `Option<(u16, u32)>`, keeping the
one-in-flight property honest under an unrelated cancel), +8 B → `App`
37,040 B. The BLE profile additionally pays **72 B** for the two bounded
lossless delete `Channel`s (8 × `u16` slots each) that replace the overwriting
route/ride delete `Signal`s (finding 3). On pinned rustc 1.96.0, default grows
**201,944 → 202,992 B** resident and BLE **213,864 → 214,984 B**, the new
approved ceilings. The floating-stable rustc 1.97.1 (`8bab26f4f 2026-07-14`) CI
run before the review round linked default at 202,976 B (the repository's 8 B
default toolchain margin) and BLE exactly at its then-ceiling, with 658,484 B /
1,084,052 B flash; the pinned observations in the table are 670,168 B and
1,110,228 B. Framebuffer count, `.uninit`, and the guarded poll frames
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

### The upload retune's staging halves (#1158 follow-up), 2026-08-07 — **0 B of anything**

The USB staging arm goes 16 KiB → **two 32 KiB halves** (`usb::STAGE_HALF_LEN`), so the transport
fills one while the other is the card's, and each flush is exactly one FAT cluster — one 64-block
CMD25 with no partial ends. The interesting part of the entry is that there is nothing to pay:

| | before | after |
| :-- | --: | --: |
| `compile_time_allocations.arena_usb` | 16,384 B | **65,536 B** |
| `arena_total` (= `max(arms)`, render) | 117,408 B | 117,408 B |
| `.bss + .data` | 295,816 B | 295,816 B |
| `.uninit` | 118,432 B | 118,432 B |
| largest guarded poll frame | 9,728 B | 9,728 B |
| residual main stack | 77,272 B | 77,272 B |

**This is the growth asymmetry working exactly as #1146 P2 documented it.** The arena is
`max(arms)`, not their sum, so an arm below the maximum grows free until it reaches the render
arm's 117,408 B — and 65,536 is still comfortably under. Measured locally as this branch against
the same tree with the halves back at 16 KiB: every figure above is byte-identical, so the only
line that moves in the JSON is `arena_usb` itself. The `#[inline(never)] drain_bulk_out` future the
same branch adds to the USB data plane lands inside existing slack: `task_frame_measured` is
unmoved at 7,328 B.

The trap this entry exists to close is the *old* argument, which read "16 KiB takes 94% of the
batching win and a second 16 KiB would halve the margin over the 35,808 B deep-ride peak". That was
a **`.bss`** argument, written when the staging buffer was a static of its own, and #1146 P2
retired it by moving those bytes into the arena. It survived in `usb::STAGE_LEN`'s doc for two
epics; it is gone now, and the doc says what actually bounds the constant (the card's cluster size,
then the render arm).

**Two report rows were also added rather than dropped.** The sEMMC pivot pinned `sd_bounce` and
`semmc_driver` in `resource_baseline.json` but never added them to
`main.rs`'s `resource_report::OBC_RESOURCE_TABLE`, so `resource_guard.py report` has failed
"missing entries" on every build since. Both are real resident blocks the pivot introduced and the
report exists to make exactly that kind of block legible by name, so the table grew the rows.
`semmc_driver` re-pins 52 → **44 B** at the same time: `size_of::<Semmc>()` links 44 on the current
tree and the baseline was carrying the pivot's figure. Neither is this branch's own cost.

### The sEMMC carve (#1158 PR1), 2026-08-06 — **0 B of `.bss`/`.uninit`, −20,480 B of stack**

The first change that costs RAM without linking a single byte. The microSD host moves off SPI
onto Nordic's sEMMC soft peripheral, which the FLPR executes from its own **permanent** carve —
permanent because storage reads happen mid-render, which rules out the #1146 arena. `build.rs`
places it directly below the display FLPR's carve and shrinks the M33's linked `RAM` to match:

| | |
| :-- | --: |
| image (code 15,360 + exec/data 1,536 + VRI 512) | 17,408 B |
| reserved carve (4 KiB-aligned, `0x2007_8000 .. 0x2007_D000`) | 20,480 B |
| M33 `RAM` | 500 KiB → **480 KiB** |

- **Linked resident is unchanged** on both profiles — `.bss + .data` 293,688 B and `.uninit`
  118,432 B to the byte. The carve is not a static; it is RAM the linker never sees. (The driver
  itself is dead code in this PR, so even its ~32 B of statics are absent; expect a few dozen
  bytes when the integration PR wires it up.)
- **`residual_stack` pays all of it**, 99,880 → **79,400 B**, −20,480 exactly, because the bound
  is `_stack_start − __euninit` and `_stack_start` is the top of the `RAM` region. This is the
  gate that catches a carve, and the only one — a reviewer reading `.bss + .data` alone would see
  nothing at all.
- Against the measured deep-path peak (35,808 / 37,760 B) that still leaves ~42 KB, and against
  the boot-chain ceiling (28,116 B, unchanged) 51,284 B — an order of magnitude over the 4,096 B
  `boot_chain_headroom_min` floor. `main.rs`'s compile-time budget assert reads the same
  generated `M33_RAM_BYTES` and clears with ~13.8 KB of margin.

**Where the 20 KB came from.** #1146's net ~51 KB win, of which P3 had already spent ~25 KB on
the frame caps; this is the rest of it, and it is what made the carve possible at all (the carve
without #1146 HardFaulted on the first deep render — measured, #1145). 2,560 B of the region is
slack: 17,408 rounds up to 20,480 to keep the image 4 KiB-aligned, the alignment every on-glass
measurement was taken at. If Nordic documents a weaker `INITPC` alignment those bytes come back.

### The frame caps spend the dividend (#1146 P3), 2026-08-05 — **+25,088 B of `.uninit`, 0 B of `.bss`**

The other half of the arena trade, and the first change whose whole cost lands in `.uninit`.
`obc-render`'s per-frame caps grow into the render arm: `MAX_SPANS` 1,152 → 1,792,
`MAX_FRAME_POINTS` 4,768 → 6,208, `MAX_FRAME_RINGS` 1,024 → 1,792, `MAX_CROSSINGS`
256 → 640. `size_of::<RenderScratch>()` — which *is* `arena_render` and, the render arm
being the maximum, also `arena_total` — goes 92,320 → **117,408 B**, exactly the sum of the
four deltas with no padding shift.

What moved and what did not, on both profiles:

- **Linked resident is unchanged at 293,688 B.** Not a rounding accident — the bytes live in
  `.uninit`, so this is precisely the case P2 predicted the `.bss + .data` gate would sleep
  through. `uninit_max` is what catches it: 93,344 → **118,432 B**. (The base is 293,688 and
  not P2's 293,784 because #1143's cleaning took 96 B out of `App` underneath this branch;
  that 96 B is its note's, not P3's.)
- **`residual_stack` pays the same 25,088 B**, 124,968 → **99,880 B**, because the bound is
  `_stack_start − __euninit`. Against the measured deep-path peak (35,808 / 37,760 B) that
  still leaves ~62 KB, and the boot-chain ceiling (28,116 B pinned host) ~71 KB.
- **Neither smaller arm changed, and both got cheaper to grow.** Free headroom before an arm
  would cost a byte: nav 59,872 B → ~57.5 KB of room, USB 16,384 B → ~101 KB.

**The net, which is the point.** The three blocks used to sum to 168,576 B of permanent
residency; the arena costs `max(arms)` = 117,408 B — **51,168 B less even after the spend**.
Measured end to end against pre-P2 develop it is 51,296 B: total RAM footprint
(`.bss + .data + .uninit`) 463,416 → 412,120 B, and the residual main stack 48,584 →
99,880 B — the same number arriving from the other side. **Both are correct and neither
derives the other** — they differ by 128 B, and every byte of the 128 has a name: P2's own
40 B accounting gap (its 76,256 by max-of-arms against 76,296 linked: the arena's
8-alignment in `.uninit` plus `.bss` repadding), less the 8 B #1150's review round put *into*
`App`, plus the 96 B #1143's cleaning took *out* of it. The linked figure charges all three;
the arms sum charges none. 40 − 8 + 96 = 128. The check they provide is mutual corroboration
to within named linker padding, not an identity.

Renders change only where the old caps were doing the dropping. `obc-bench`'s `overview` /
`overview-rot` now draw 1,792 of 3,625 features (was 1,024; 2,601 → 1,833 dropped) and their
two frame hashes were re-blessed; `riding` / `mid` / `route` are byte-identical, as are
headless sim frames at every zoom that never saturated.

**Rings, not spans, are the feature ceiling** — the finding this PR's review round proved and
then acted on. `select()` budgets points and rings and never counts spans, and every admitted
feature is charged at least one ring (`Kind::Line` included; a candidate with no rings is
rejected by `Feature::has_valid_rings`, and `ring_count == 0` is pass-B's failure sentinel), so
a frame draws at most `min(MAX_SPANS, MAX_FRAME_RINGS)` features. On develop that minimum was
`MAX_FRAME_RINGS`: every saturating A/B frame reads *rings 100 % / spans 89 %*. Hence the final
split — the ring cap rose to meet `MAX_SPANS` at 1,792, funded by trimming `MAX_FRAME_POINTS`
1,536 B, which leaves the arm cost-neutral at 117,408 B and makes the whole span reservoir
reachable instead of dead weight. A `const` assert in `obc-render` now refuses the inverted
shape. Points did not become the new limiter: the busy frames measure 2.93–3.01 points per
admitted feature at the new caps (3.12 is the highest reading anywhere in the A/B, and it is on
the *old*, tighter ring ceiling) against the 6,208 / 1,792 = 3.46 the budget now allows —
`obc-bench`'s `overview` now saturates rings *and* spans at 100 % with points at 87 %.

The frame-time price of the extra features is on the host bench, `overview` at develop's caps
against these (min-of-10 per stage, same host): 1,024 → 1,792 features drawn (+75 %) costs
collect 281 → 359 µs, sort 15 → 28 µs, draw 191 → 255 µs, total 488 → 643 µs (+32 %). Frame time
grows sub-linearly in features. It is a host figure on an in-memory fixture and captures none of
the device's SD-read cost, which dominates a real wide frame; the on-glass number is still owed.

### The scratch arena (#1146 P2), 2026-08-05 — **−76,296 B resident; `.uninit` becomes load-bearing**

Three blocks that are never live at once — the ~90 KB render scratch, the ~58 KB nav
block (`NavScratch` + `NavTileCache` + `NavPlanner`; the terrain sampler stays out, it is
read at fix cadence), and the 16 KiB USB staging buffer — now time-share one
`ScratchArena` union (`arena.rs`, the only place the feature's `unsafe` lives). Owner
bookkeeping is the host-tested `obc_app::ArenaGate`; the product rules that make the arms
disjoint (the Recalculating freeze, the transfer screen, the transfer gate's new
`search ⊕ transfer` arm) live in `obc-app` where they are unit-tested.

Two accounting consequences outlive the feature:

- **The budget is max-of-arms, with cliff semantics.** The arena costs
  `size_of::<ScratchArena>()` = the largest arm (92,320 B, render). Growing any smaller
  arm is **free** until it crosses the max arm — nav has ~32.4 KB and USB ~75.9 KB of
  free headroom — and growing the max arm costs 1:1. Stated at the budget assert so
  neither a free growth is "optimized" nor a 1:1 one waved through.
- **`.uninit` is no longer a 1 KB sidelight.** The arena lives there
  (`.uninit.OBC_SCRATCH_ARENA`, never zeroed at boot — arms init in place on claim), so
  `uninit_max` is now a real growth gate (93,344 B = arena + defmt's ring), and the RAM
  story is `.bss + .data` *plus* `.uninit`: the stack boundary (`_stack_end = __euninit`)
  charges both. "Linked resident" in the CI contract remains `.bss + .data`.

Gate figures moved with it, measured on the pinned host (whose merge-base build
reproduces the old baseline exactly): linked resident 462,392 → 293,776 B, residual main
stack 48,584 → **124,880 B** (the net drop, to the byte), largest task body
20,352 → 6,912 B (the old figure carried a ~13.5 KB incidental `memcpy` the P2 codegen
no longer emits — the tightened 8,192 B limit will correctly fire if it returns),
boot-chain ceiling re-pinned 29,696 B against a `boot_chain_measured_ci` of **21,884**,
re-pinned from P2's own `embedded` run — ~6.2 KB under the pinned host's 28,116, the host
spread P1 recorded. The two full-frame-sized writables are now `FB` + `ARENA`.

The review round on that PR moved the resident figures 8 B: `App` grew by three `bool`s
(the freeze's two per-family plan levels + its engaged-level repaint bit, and the matcher's
one-shot wide re-lock), so linked resident is **293,784 B** and the residual stack
**124,872 B**. No arm of the arena changed size.

### Boot-path stack gates (#1108 follow-up), 2026-08-03 — **0 B resident, four new gates**

The re-baseline that exists because the guard **missed a boot-bricking regression**. Every
image built from develop between #1084 (elevation EL7) and #1108 hard-faulted at boot — a
stack overflow escalated to HardFault at `link::init_store`'s prologue, before either link
plane spawned — and the whole `embedded` matrix stayed green through it.

The gap was `poll_frame_limit`'s symbol match. It measures every
`TaskStorage<F>::poll` prologue, but the embassy **main task's** frame is allocated in the
out-of-line `____embassy_main_task::____embassy_main_task_inner_function::{{closure}}`,
which that match excludes. EL7 inlined a `TerrainElevation::parse` chain into the ride
task's async block, and because a non-await-crossing temporary in an async fn is a
*permanent* poll-frame slot (not the transient the code comment claimed), the main task's
frame grew 20,352 → 22,400 B entirely unobserved. Three individually reasonable changes
then summed past the stack: that +2,048 B, `init_store`'s pre-existing 27.7 KB
double-copy, and the epic's legitimate +3.7 KB of `.bss` — which is also a **stack cut**,
since the statics grow up towards a stack that grows down.

| | 5de00ce (bricked) | #1108 (`b7405542`) | gate |
| :-- | --: | --: | :-- |
| largest out-of-line task body | 22,400 B | **20,352 B** | `task_frame_limit` 21,504 B |
| residual main stack (`_stack_start − __euninit`) | 48,600 B | 48,600 B | `residual_stack_min` 48,600 B |
| boot-chain ceiling | 56,532 B | **41,556 B** pinned host / **35,356 B** CI | `boot_chain_ceiling` 43,008 B |
| boot-chain headroom | **−7,932 B** | **+7,044 B** pinned host / **+13,244 B** CI | `boot_chain_headroom_min` 4,096 B |
| largest guarded poll frame | 9,728 B | 9,728 B | 12,288 B, unchanged |
| linked resident | 462,376 B | 462,376 B | unchanged — this is a tooling change |

The first two gates are **exact measurements** and either one alone fails the bricked
image. The **boot-chain ceiling is deliberately conservative**: the walk follows every
direct `bl` edge whether or not the path is feasible, so a `Mode::ReadOnly`
`open_file_in_dir` monomorphization still drags in `alloc_cluster`/`update_fat` frames a
rescan can never execute, and indirect calls (`blx`, `dyn` dispatch) are invisible to it.
It is therefore a **drift detector, not a stack-safety proof** — the on-glass high-water
below remains the only authority on real headroom. `boot_chain_headroom_min` earns its
place as the invariant tying the other two together: ceiling and residual are
independently re-approvable numbers, and #1084 failed precisely because separately
reasonable changes summed, so the combination needs a gate of its own.

`boot_chain_roots` names the `#[inline(never)]` boot constructors (`link::init_store`,
`mount_terrain`) as substrings, so the mangling hash is not pinned. A root that stops
resolving is a **hard error, not a skip** — "renamed, or inlined away" covers the #1084
mechanism itself, where losing the attribute moves a fat temporary into a permanent frame.

**The ceiling figure is toolchain-sensitive; the task-frame figure is not.** Measured on
this change's own CI run
([30852527404](https://github.com/timohueser/OpenBikeComputer/actions/runs/30852527404)),
floating stable links a 35,356 B ceiling against the pinned host's 41,556 B — 6.2 KB apart,
because the walk's depth depends on which callees survive as distinct symbols under
inlining. `boot_chain_ceiling` is set above **both** so neither host false-fails, so its
slack differs per host (~7.6 KB on CI, ~1.4 KB locally). That is tolerable precisely
because it is the drift detector: `task_frame_measured` reads exactly **20,352 B on CI and
on the pinned host**, so the gate that actually catches the #1084 class is
host-independent. Re-pin both figures from an `embedded` run, never from a laptop.

Baseline `schema_version` goes to **2** for the new keys. `resource_guard.py board` runs
these gates on both profiles in the existing CI steps; no workflow change was needed.

### Device-side map storage (#927), 2026-07-27 — **+48 B resident, taken from CI**

The first re-baseline whose numbers come from **CI rather than the pinned host**, and the
change is small enough to say why plainly: the `embedded` job is the gate, it runs
floating stable, and it linked exactly the recorded 421,176 B on develop `9bb7adf8`
(run [30300503081](https://github.com/timohueser/OpenBikeComputer/actions/runs/30300503081))
— so its 421,224 B on this branch is a clean, attributable **+48 B**. The pinned 1.96.0
host links **+40 B** for the identical tree (416,040 → 416,080 `.bss`); the 8 B gap is
toolchain layout, not a second change, and pinning the ceiling to the *lower* of the two
would red the gate on every CI run.

| | develop `9bb7adf8` (CI) | with #927 (CI) | Δ |
| :-- | --: | --: | --: |
| `.bss` | 416,040 B | 416,088 B | +48 B |
| `.data` | 5,136 B | 5,136 B | 0 |
| **linked resident** | **421,176 B** | **421,224 B** | **+48 B** |
| `.uninit` | 1,024 B | 1,024 B | 0 |
| flash | 1,166,192 B | 1,185,668 B | +19,476 B |
| largest guarded poll frame | 9,728 B | 9,728 B | 0 (limit 12,288 B) |

Itemized per symbol from an `llvm-nm -S` diff against `9bb7adf8` on the pinned host (the
one place a stable-hash-free comparison was available), which accounts for the +40 there
and, symbol for symbol, for the +48 on CI:

| symbol | before | after | Δ | what it is |
| :-- | --: | --: | --: | :-- |
| `link::MAP_PHASE` | — | 1 B | +1 B | the map-transfer progress mirror the ride loop |
| `link::MAP_RX_KIB` | — | 4 B | +4 B | polls once per pass to drive the on-glass card — |
| `link::MAP_TOTAL_KIB` | — | 4 B | +4 B | the `RECORDING` pattern, three relaxed cells |
| `SHARED_STORE` | 5,716 B | 5,728 B | +12 B | `Storage::open_map_name: Option<ShortFileName>` |
| `__embassy_main::POOL` | 31,216 B | 31,240 B | +24 B | the ride loop's map-card reconcile block |
| `.L_MergedGlobals` (all) | 869 B | 880 B | +11 B | the compiler's small-static pool, re-laid out |

The `Storage` field is the one worth a sentence: embedded-sdmmc refuses every second open
of an open file, so the map catalog has to read the **loaded** map's header through its
live handle — and to recognise which scanned entry that is, it needs the open map's 8.3
name. Without it the loaded map would be the one map missing from its own catalog, which
is exactly the trap issue #480 documents for route downloads.

**No named compile-time allocation moved.** `app` is still 135,224 B and
`ble_object_store` still 13,528 B, verified against the report ELF on both profiles: the
feature added module statics and a single `Storage` field, and its new `Screen` variant
(`MapTransfer`, 12 B of state) is far smaller than the enum's largest, so
`size_of::<App>()` is untouched. That is the discriminator this document already states —
module statics move the `.bss` gate only; fields of `App` / `ObjectStore` move the report
as well — landing on the expected side for once.

Flash is not gated on this target, but +19 KB is worth naming: the new `sd.rs` map path
and catalog scan, the map-transfer screen and its four-language copy, and the `obc-ble`
announce/held-magic codecs.

“Linked resident” is the CI contract's `.bss + .data`. `.uninit` is reported
separately. The M33 receives 253,952 B after the FLPR carve, leaving 51,008 B
after default `.bss + .data + .uninit` and 39,088 B after BLE. Those residuals
are linker headroom, not measured stack high-water. The independent 36,864 B
`STACK_RESERVE` is a compile-time floor, also not a runtime measurement.

The framebuffer guard requires exactly one `FB` symbol of 240 × 320 × 1 byte
and exactly one writable symbol at least that large. Both shipping profiles'
disassembly guards check every `TaskStorage<F>::poll` prologue and retain the
12,288 B safety ceiling; the current maxima are 52 B for default and 6,240 B
for BLE. Since the #1108 follow-up they **also** check the out-of-line
`____embassy_*_task` bodies, which `TaskStorage<F>::poll` does not cover and
where the main task's real frame lives, plus the residual stack and the
boot-chain ceiling/headroom — see the 2026-08-03 entry above for why.

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

The checker covers the root `Cargo.toml` workspace plus the standalone
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
| USB enumeration + upload-while-riding contention (#889) | **PENDING — owner device capture** |
| default / BLE stack high-water | **PENDING — owner device capture** |

The PR carrying this baseline is not device-verified and must not merge until
the owner replaces these `PENDING` cells with captured evidence or explicitly
records an approved follow-up disposition.
