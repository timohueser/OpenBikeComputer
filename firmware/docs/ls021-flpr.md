# LS021B7DD02 FLPR backend — toolchain, memory & boot spec (STATUS: F1 LANDED)

The **normative reference** for moving the Sharp **LS021B7DD02** waveform generation off
the Cortex-M33 and onto the nRF54L15's **FLPR** (the VPR RISC-V coprocessor). Epic [#149];
this doc is the deliverable of **F0 [#150]** and the spec the firmware of F1–F5 is written
against. It is the FLPR-epic analog of `ls021-bringup.md` (the M33-direct epic #139 spec),
which stays the **golden reference**: the FLPR must reproduce that analyzer-verified frame.

- **F0 [#150]** — toolchain + blob boot + this spec; the FLPR toggles a GPIO. ✅ **DONE**
  (build/boot path stood up; FLPR blinks LED0 + answers a shared-RAM handshake — see
  [Verification](#verification)).
- **F1 [#151]** — M33↔FLPR comms: shared control block + a doorbell each way, round-trip verified.
  ✅ **DONE** (the one-word handshake is now a structured control block; M33→FLPR is a shared-RAM
  sequence, FLPR→M33 is an EGU interrupt — VEVIF turned out walled on bare metal; the M33 sweeps
  commands and verifies the round-trip over RTT — see [M33 ↔ FLPR comms](#m33--flpr-comms)).
- **F2 [#152]** — FLPR drives one source sub-line from a write buffer (LA diff vs M33).
- **F3 [#153]** — FLPR drives a full frame (init-black + solid colour on glass).
- **F4 [#154]** — ping-pong write buffers + pack from the RGB222 framebuffer (palette + shapes).
- **F5 [#155]** — `obc_platform::Panel` backend + speed-tune toward the ~53 ms spec frame.

> **What F0 proves, and why it's first.** The whole epic rests on one untested assumption:
> that we can *build* code for the FLPR, *boot* it from the M33, and have it *drive a pin*.
> F0 isolates exactly that — no panel signal, no `PanelBus`, no COM. If any of build / boot /
> GPIO-access were going to bite, it bites here, cheaply, with two LEDs and an RTT line.

## Architecture (recap from the epic)

```
   M33 (Cortex-M33)                         FLPR (RV32EMC coprocessor)
   ────────────────                         ──────────────────────────
   build.rs cross-compiles the C blob ─▶ include_bytes! ─▶ copy to FLPR RAM
   set VPR00.INITPC + CPURUN.EN=1  ───────────────────────▶  FLPR runs from INITPC
   write MAGIC to the shared word  ◀── shared SRAM ──▶       FLPR writes ALIVE
   poll the shared word → RTT "alive"                        FLPR toggles GPIO (source bus, later)
   COM (VCOM/VB/VA) free-runs on the M33  ── independent of the FLPR (safety; epic decision)
```

- **FLPR firmware = C** (epic decision). The VPR is **RV32E + M + C**; Rust's
  `riscv32emc-unknown-none-elf` is tier-3, so the blob is a small freestanding C program built
  with a RISC-V gcc, emitted to a `.bin`, and `include_bytes!`'d into the M33 image. All
  non-trivial logic (the RGB222→wire pack) stays host-tested Rust on the M33 (F4).
- **COM stays on the M33.** The proven L1 timer task (`ls021.rs::com_task`) keeps `VCOM`/`VB`/
  `VA` free-running independent of FLPR state — if the FLPR faults the panel never takes a DC
  bias. The FLPR owns gate + source only. F0 runs neither (no panel signal yet).

## Toolchain

**Any `rv32emc`-capable GNU gcc** works — the blob is freestanding (`-nostdlib
-nostartfiles`, integer ops only), so **no libgcc / newlib / rv32e multilib is linked**; only
the compiler's code-gen for `-march=rv32emc -mabi=ilp32e` is used. The repo's choice is
Homebrew's `riscv64-elf-gcc`:

```sh
brew install riscv64-elf-gcc        # GCC 16.x, bottled; same formula family as riscv64-elf-gdb
```

`build.rs` (under the `ls021-flpr` feature only) compiles `src/flpr/` into `$OUT_DIR/flpr.bin`.
The exact invocation it runs:

```sh
riscv64-elf-gcc -march=rv32emc -mabi=ilp32e \
    -Os -ffreestanding -nostdlib -nostartfiles -fno-pic \
    -ffunction-sections -fdata-sections -Wall -Wextra -Wl,--gc-sections \
    -T src/flpr/flpr.ld src/flpr/start.S src/flpr/flpr_blink.c -o flpr.elf
riscv64-elf-objcopy -O binary flpr.elf flpr.bin        # raw image, no ELF headers
```

`build.rs` probes `RISCV_GCC` (override) → `riscv64-elf-gcc` → `riscv-none-elf-gcc` →
`riscv64-unknown-elf-gcc`; the objcopy is the gcc's `-gcc`→`-objcopy` sibling (or
`RISCV_OBJCOPY`). If none is found it fails with an install hint. The carve-out + blob build
are **feature-gated**, so a plain `cargo build` (the `main.rs` map firmware) needs **no
RISC-V toolchain** and keeps the full 256 KB.

> **Why this triple, not the xPack `riscv-none-elf-gcc` the epic named.** Functionally
> identical GNU toolchains; `riscv64-elf-gcc` is a one-command Homebrew install on macOS (and
> `gcc-riscv64-unknown-elf` / equivalent on Linux CI). The `RISCV_GCC` override lets the xPack
> or Zephyr-SDK toolchain drop in unchanged.

## FLPR memory map (nRF54L15, 256 KB SRAM @ 0x2000_0000)

The FLPR executes from on-chip SRAM at the **M33-visible address** (no remap on this part):
the M33 copies the blob to the region base and writes that base into `INITPC`. Nordic's
guidance is **≤96 KB to the FLPR**. F0 reserves a generous-but-modest top slice; the
production backend (F4/F5) will shrink it (see the coexistence note). The carve-out is emitted
by `build.rs` **only under the `ls021-flpr` feature** (Cargo's `CARGO_FEATURE_LS021_FLPR`), so
`main.rs` is untouched.

| Region | Range | Size | Owner / contents |
|---|---|---|---|
| `RAM` | `0x2000_0000 .. 0x2003_8000` | 224 KB | **M33** `.data`/`.bss`/stack (the linked `RAM`) |
| `FLPR_RAM` | `0x2003_8000 .. 0x2003_F000` | 28 KB | **FLPR** image + stack; `INITPC = 0x2003_8000`, `_stack_top = 0x2003_F000` |
| `SHARED` | `0x2003_F000 .. 0x2004_0000` | 4 KB | **cross-core** control block (F1 — the 64-byte `Control`/`flpr_control_t`; see [comms](#m33--flpr-comms)) |

- The map lives in **two places that must agree**: `build.rs`'s carved `memory.x` (shrinks the
  M33 `RAM` to 224 KB) and `src/flpr/flpr.ld` (places the FLPR image at `0x2003_8000`,
  `_stack_top` at `0x2003_F000`). The M33 reaches `FLPR_RAM`/`SHARED` **only by hardcoded
  address** (`memcpy` + the handshake word in `ls021_flpr_bringup.rs`), never via the linker —
  so shrinking `RAM` is the entire M33-side change.
- **The FLPR region is `rwx` to the FLPR but the M33's `RAM` stops at `0x2003_8000`**, so the
  M33 linker never allocates into it. The M33 *can still write* there (it's plain SRAM) — that
  is exactly how it loads the blob and the handshake.

> **Coexistence with the 75 KB framebuffer (F4/F5, flagged not solved).** The production map
> firmware's resident set (`App` scratch + caches + the 75 KB `FbDevice64` + stack) already
> fills ~254 KB of the 256 KB (the `main.rs` `nrf-mem` budget assert, issue #124). The FLPR
> backend's real need is small — the blob (~few KB) + two write buffers (~1 KB) + FLPR stack —
> so F4/F5 will reserve a **much smaller** `FLPR_RAM` (≈8–16 KB) and retune `nrf-mem` to free
> it. F0's 28 KB is bring-up headroom, not the final budget. This is the "pin the memory map"
> task the epic called out.

## Boot sequence (M33 launcher)

Via the **VPR00** peripheral (secure alias base `0x5004_C000`; offsets from the nRF54L15 PAC):

| Register | Address | Use |
|---|---|---|
| `CPURUN` | `0x5004_C800` | bit0 `EN` = 1 → FLPR runs after core reset |
| `INITPC` | `0x5004_C808` | initial PC at start = FLPR exec base |

The M33 (`ls021_flpr_bringup.rs::start_flpr`) does, in order:

1. `copy_nonoverlapping(blob, 0x2003_8000, blob.len())` — load the image.
2. `dsb()` — make the blob **and** the pre-written handshake magic visible to the other core
   before release.
3. `INITPC = 0x2003_8000`.
4. `CPURUN = 1` — the FLPR begins executing at `INITPC` (i.e. `_start` in `start.S`).

`_start` sets `sp = _stack_top`, zeroes `.bss` (empty for the F0 blob), and calls
`flpr_main()`. Done as **raw register writes** so it depends on neither an embassy VPR driver
nor the secure/non-secure PAC alias choice.

## Pin ownership & the shared-P2 rule

`P2` is the FLPR's **dedicated** GPIO domain (Nordic: TRACE/FLPR pins live on P2) — and it is
where the LS021 **source bus + `BCK` + COM** all sit. Two cores will touch P2 at once (M33
COM, FLPR source), so the epic's rule is **absolute**:

> **Never read-modify-write `GPIO.OUT` from either core. Use only the atomic `OUTSET`
> (`+0x04`) / `OUTCLR` (`+0x08`) set/clear registers.** Each is a single write of a pin mask;
> the two cores' set/clears on disjoint pins never corrupt each other.

- **Pin *configuration*** (direction, drive) is owned by the **M33** — it calls embassy
  `Output::new` once and keeps the handle alive. The FLPR only ever pulses `OUTSET`/`OUTCLR`.
- **F0 pins:** LED0 = **P2.09** (FLPR-driven, the source-port proof + visual check); LED1 =
  **P1.10** (M33 heartbeat). GPIO P2 secure base `0x5005_0400` (`OUT 0x00`, `OUTSET 0x04`,
  `OUTCLR 0x08`, `DIR 0x10`, `DIRSET 0x14`); LED0 mask = `1<<9`.

> **SPU / secure-GPIO fallback.** F0 drives the **secure** P2 alias (`0x5005_0400`), matching
> the all-secure `nrf54l15-app-s` build. If the FLPR boots (handshake fires) but LED0 stays
> dark, the FLPR lacks secure-GPIO access: try the **non-secure** alias (`0x4005_0400`) in
> `flpr_blink.c`, and/or grant the FLPR access to GPIO P2 via the SPU `PERIPH[n].PERM`
> ownership the way the M33 build already needs the Board-Configurator ext-mem-off / 3.3 V
> VDDM settings. Discovering which is needed is part of what F0 verifies; record the answer
> here when the board confirms it.

## M33 ↔ FLPR comms

F0 used the crudest possible channel: one shared word (M33 writes magic, FLPR overwrites with
`0xA11E`, M33 polls). **F1 ([#151])** replaces it with a **structured control block** in the
`SHARED` page plus a **doorbell in each direction** — the comms analog of the M33 bring-up's L1,
round-trip verified before any panel signal. The ping-pong write-buffer handoff (F4) reuses this
block and these doorbells.

> **The epic named VEVIF; the silicon said no.** The plan was to use the VPR's VEVIF mailboxes
> both ways. On this **bare-metal** setup (no Zephyr/sysbuild VPR runtime) both directions turned
> out to be walled — see [Why not VEVIF](#why-not-vevif) below for the full on-glass story. F1
> ships the channel on mechanisms that **do** work here: a **shared-RAM sequence** for M33→FLPR
> and an **EGU interrupt** for FLPR→M33. Both are exactly what F4 needs (the FLPR polls a
> buffer-ready flag; the M33 sleeps until an IRQ says a buffer drained).

### Control block (64 bytes at `0x2003_F000`)

Normative layout, **identical** in `Control` (Rust, `ls021_flpr_bringup.rs`) and `flpr_control_t`
(C, `flpr_comms.c`) — both static-assert the 64-byte size. `#[repr(C)]` + all-`u32` members ⇒ no
padding, deterministic offsets. Little-endian; every field is accessed `volatile` (the cores write
it concurrently).

| off  | field         | writer | meaning |
|------|---------------|--------|---------|
| 0x00 | `magic`       | M33    | layout/version tag `0xF1C0_0001`; the FLPR refuses to act if it mismatches |
| 0x04 | `m33_seq`     | M33    | command sequence counter — **the M33→FLPR doorbell** (bumped last, after `cmd`) |
| 0x08 | `cmd`         | M33    | command word (F1: the value `N`) |
| 0x0C | `flpr_seq`    | FLPR   | echoes the `m33_seq` it serviced (round-trip proof) |
| 0x10 | `status`      | FLPR   | ack/result (F1: `cmd ^ 0xA11E`; boot: `0xA11E` alive, `0x0BAD_CAFE` = magic mismatch) |
| 0x14 | `frame_count` | FLPR   | frames drained (F4; defined now, unused in F1) |
| 0x18 | `buf[0..2]`   | both   | `BufDesc { ptr, len, ready, consumed }` ×2 (16 B each) — F4 ping-pong descriptors |
| 0x38 | `reserved[2]` | —      | forward-compat headroom |

### Doorbells

| direction | mechanism | detail |
|---|---|---|
| **M33 → FLPR** | shared-RAM **sequence** | M33 writes `cmd` then bumps `m33_seq` (+`dsb`); the FLPR polls `m33_seq` and services on a change. The FLPR is a dedicated core, so polling is correct — and this is exactly F4's "buffer ready" handshake. |
| **FLPR → M33** | **EGU20** interrupt | the FLPR writes `EGU20.TASKS_TRIGGER[0]` (secure `0x500C_9000`) — a plain peripheral store, like driving GPIO. `EGU20.EVENTS_TRIGGERED[0]` raises the M33's **`EGU20` IRQ #201**; the ISR (`Priority::P3`) clears the event + signals an `embassy_sync::Signal`. No pin, and the M33 sleeps instead of busy-waiting. |

EGU is the nRF "software interrupt" peripheral and a normal, M33-writable peripheral (its `INTEN`
accepts writes — the crux, see below). **Reserve `EGU20`** when this folds into the real firmware
(F5) so nothing else claims it; `EGU10` is the spare alternative.

### Why not VEVIF

Mapped on glass, both VEVIF directions are unusable from the bare-metal secure M33:

- **First**, *any* VEVIF CSR access on the FLPR (`csrr 0x7e0` etc.) **faults** until RT peripherals
  are unlocked: write `VPRNORDICCTRL` (CSR `0x7c0`) = `NORDICKEY 0x507D`<<16 | `ENABLERTPERIPH` (=
  `0x507D_0001`). Skipping this froze the FLPR right after the ALIVE stamp. (Cross-checked vs
  Nordic's `nrf_vpr_csr_rtperiph_enable_set`.)
- **M33 → FLPR (VEVIF task):** even with RT peripherals unlocked, `mstatus.MIE`=0, and the task's
  `INTEN` CSR (`0x7e4`) enabled, an M33 `TASKS_TRIGGER[0]` write never latched into the FLPR's
  readable `TASKS` CSR (`0x7e0`) — the FLPR never saw the doorbell.
- **FLPR → M33 (VEVIF event):** the FLPR's `csrs 0x7e2` *does* set the app's `EVENTS_TRIGGERED[16]`,
  but it can't be gated to the NVIC — writing the app-side `VPR00.INTEN`/`INTENSET`
  (`0x5004_C300/304`) **does nothing** (reads back `0`; an all-ones write yields `0`), so the
  interrupt never fires. The non-secure alias (`0x4004_Cxxx`) BusFaults, confirming VPR00 is
  secure-only. Arming VPR00's app interrupt needs SoC-level init Zephyr does and we don't.

Lesson (echoing L1's "PWM won't route to the COM pins"): on this part, *compiles + runs* ≠
*routes*. The shared-RAM + EGU path sidesteps all of the above and is verified end-to-end.

### Memory ordering

A flag must never be observed before the data it guards, across cores. The rule, both directions:
the writer fills the data fields, then writes the **sequence field last** as the guard, with a
**barrier before signalling** — the M33 uses `cortex_m::asm::dsb()`, the FLPR a RISC-V `fence`.
Concretely: M33 writes `cmd` then `m33_seq` (+`dsb`); the FLPR, on seeing `m33_seq` change, writes
`status` (+`fence`) then `flpr_seq` (+`fence`) then pokes the EGU. The FLPR blob uses **no CSRs**
(only `fence`, base ISA), so the blob builds with plain `-march=rv32emc` — no Zicsr.

## Verification

F1 subsumes F0's boot proof (the FLPR still copies in, boots, and reaches shared SRAM — now it
stamps `status = 0xA11E` in the control block instead of a lone word) and adds the round-trip:

- **Build (host/CI):**
  - `cargo build --release --bin ls021_flpr_bringup --features ls021-flpr` — the blob compiles
    (`rv32emc`, no Zicsr), `objcopy` runs, the carved `memory.x` links, and `flpr.bin` (~214 B for
    F1) embeds.
  - `cargo build --release` (default `main.rs`) — the feature gate left the 256 KB budget assert
    intact and needs **no** RISC-V toolchain (regression guard).
- **On glass (RTT + eye):** `cargo run … --features ls021-flpr` →
  - RTT logs **`EGU20 armed ch0 — INTEN=0x00000001`** (the EGU interrupt-enable latches — unlike
    VPR00's) then **`FLPR alive`** (`status` read `0xA11E`) → the FLPR booted, reached shared SRAM,
    and agreed on the control-block magic.
  - RTT then logs **`round-trip N/5 OK`** for `N = 1..=5` → each command crossed M33→FLPR
    (shared-RAM sequence), was serviced, and crossed back FLPR→M33 (EGU20 IRQ); the echoed
    `status == N ^ 0xA11E` and `flpr_seq == m33_seq`. Five values prove the channel is reliable, not
    a one-shot. A `MISMATCH`/`TIMEOUT` line prints the raw words (and, on a serviced-but-no-IRQ
    timeout, the EGU `EVENTS`/`INTEN` state).
  - **LED0 (P2.09) blinks `N` times** per command (FLPR response, by eye + LA); **LED1 (P1.10)**
    toggles per command (M33 heartbeat). After the verification sweep the M33 **loops the
    round-trip forever** (cmd cycling 1..=5) so LED0 stays continuously active for a scope/eye check
    — the FLPR only drives LED0 *while servicing*, so a one-shot sweep is a single burst that's easy
    to miss.
  - If `FLPR alive` never prints → check `INITPC`/memory map; if `status` reads `0x0BAD_CAFE` → the
    FLPR booted but saw the wrong `magic` (memory-map drift between `Control` and `flpr_control_t`).
- **Logic analyzer (optional, the golden-diff tool from epic #139):** capture **P2.09** (the FLPR's
  LED/response pulses) to confirm the FLPR services each command and the pulse count matches `N`.

## Build & flash

```sh
# From firmware/obc-fw-nrf54l/ (standalone crate, thumbv8m.main-none-eabihf). Needs a RISC-V
# gcc for the FLPR blob: brew install riscv64-elf-gcc
cargo run --release --bin ls021_flpr_bringup --features ls021-flpr
```

[#149]: https://github.com/timohueser/OpenBikeComputer/issues/149
[#150]: https://github.com/timohueser/OpenBikeComputer/issues/150
[#151]: https://github.com/timohueser/OpenBikeComputer/issues/151
