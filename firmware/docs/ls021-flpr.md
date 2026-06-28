# LS021B7DD02 FLPR backend — toolchain, memory & boot spec (STATUS: F5 implemented — bench-tune + on-glass pending)

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
  ✅ **DONE + analyzer-verified** (the inner data-shift loop runs on the FLPR — `BSP` + 124 `BCK` +
  the 6 data lines drained from a SHARED-page write buffer; the M33 hands it over through the F1
  descriptor and rings. On the LA: exactly 124 `BCK`/`BSP`, `BCK(1)` within `BSP` high, ~45 kHz
  bring-up-slow, the data pattern bit-exact, `BSP` driving on **P1** — see
  [F2 — one source sub-line](#f2--one-source-sub-line)).
- **F3 [#153]** — FLPR drives a full frame (init-black + solid colour on glass). **The "FLPR drives
  the panel" milestone.** ✅ **DONE + on-glass verified** — the FLPR now owns the *complete* waveform:
  the F2 source-shift loop wrapped in the **gate scan** (`GSP`/`GCK`/`GEN`) + **frame envelope**
  (`INTB`), ported from the M33 `PanelBus`. The M33 packs one row buffer + rings once per frame; COM
  free-runs on the M33. On glass the BTN0 white/R/G/B cycle is identical to the M33-direct L3 (#143),
  now FLPR-driven (see [F3 — full frame](#f3--full-frame-init-black--solid-colour)).
- **F4 [#154]** — ping-pong write buffers + pack from the RGB222 framebuffer (palette + shapes).
  **The epic's headline deliverable.** ✅ **DONE + on-glass verified** — two row buffers ping-pong
  (`buf[0]` even rows, `buf[1]` odd) under the per-buffer ready/consumed handshake; the M33 renders
  the palette/shapes into a resident 75 KB `FbDevice64` framebuffer and packs it a row at a time
  through the **host-tested** `obc_platform::ls021_pack_row`; the FLPR scans one buffer while the M33
  fills the other. On glass the W/R/G/B + palette + shapes cycle is identical to the M33-direct L3
  (#148), now framebuffer-sourced + FLPR-driven; RTT shows the pack overlapping the drain by ~54×
  (see [F4 — ping-pong frame](#f4--ping-pong-from-the-framebuffer)).
- **F5 [#155]** — `obc_platform::Panel` backend + speed-tune toward the ~53 ms spec frame. **The
  bridge to running the app.** 🛠️ **Implemented — bench-tune + on-glass pending** — the FLPR push
  now sits behind the board-agnostic `obc_platform::Panel` seam (`Ls021Flpr` in the bin), so the
  glass-demo generator that drives the ST7789 (`demo::font_palette_demo`) drives the real LS021 with
  no panel-specific code; the blob clocks took a first conservative speed step with the bench dial-in
  toward `BCK ≤ 0.758 MHz` documented (see [F5 — Panel backend](#f5--panel-backend--speed-tune)).

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
    -T src/flpr/flpr.ld src/flpr/start.S src/flpr/flpr_pingpong.c -o flpr.elf
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
- **F2 source-bus pins** (the [bring-up harness map](ls021-bringup.md#pinout-21-pin-fpc--dk-pin-map)).
  The **timing-critical bus is single-port P2** — `BCK` and the 6 data lines sit on `P2.00..06`, so
  the FLPR's hot 124× loop is one `OUTCLR`/`OUTSET` to present data + one to pulse `BCK`, all on the
  fast trace domain. `BSP` is the **one exception**: pulsed once per sub-line (outside the hot loop),
  it lives on `P1.07`, so F2 is also the first proof the FLPR can drive a **non-P2 (P1)** GPIO — the
  thing F3's gate scan fully depends on. GPIO P1 secure base `0x500D_8200` (same offsets as P2).

  | line | DK pin | port.bit | mask | role |
  |---|---|---|---|---|
  | `R0` `G0` `B0` (odd) | P2.00/02/04 | bit 0/2/4 | — | source data, odd pixel |
  | `R1` `G1` `B1` (even) | P2.01/03/05 | bit 1/3/5 | — | source data, even pixel |
  | `BCK` | P2.06 | bit 6 | `1<<6` | source/shift clock (FLPR's own pulse) |
  | `BSP` | **P1.07** | bit 7 | `1<<7` | sub-line start pulse (the lone P1 line) |

  The 6 data bits, pre-shifted to their P2 positions, are exactly `DATA_MASK = 0x3F` — the
  write-buffer word ([format below](#write-buffer-format-v0)). `COM` (`P2.07/08/10`, M33-driven) and
  LED0 (`P2.09`) fill the rest of P2; the FLPR's masks never touch them, so the atomic-set/clear rule
  keeps the two cores off each other's pins.

> **Product pin-planning (forward note, not an F2 constraint).** P2 has only **11 pins
> (`P2.00..10`)** — that is the whole of the FLPR's fast-toggle domain, and on the product PCB it is
> the **scarce, contended resource**. The two things that *must* toggle fast both want it: the
> display **source bus** (6 data + `BCK` = 7) and the SD-card **high-speed SPI** (4) — together
> exactly 11. That fits **only if everything slow is pushed to P0/P1**: `COM` (≤60 Hz), the gate
> lines (`GSP`/`GCK`/`GEN`/`INTB`, µs-scale), sensor **I²C** (GPS/altimeter), the **IMU SPI** can
> share or sit on P1, the **encoder**, and buttons — none need the fast domain. **USB** (future,
> nRF54LM20) is on dedicated USB pads, not GPIO, so it does not compete. The DK bench map here is
> *not* this allocation — it reuses UART/SD/flash pins because nothing else runs during bring-up
> (see the bring-up doc) — but the principle (**reserve P2 for source-bus + SD-SPI; slow signals →
> P0/P1**) is what the custom board should follow.

> **SPU / secure-GPIO fallback.** F0/F1 drive the **secure** P2 alias (`0x5005_0400`), matching
> the all-secure `nrf54l15-app-s` build, and LED0 on P2.09 confirmed the FLPR *has* secure-GPIO
> access on **P2**. **F2 extends the same question to P1** (`BSP` = P1.07): if the P2 source bus
> toggles on the LA but `BSP` stays dark, the FLPR lacks secure-GPIO access on P1 — try the
> **non-secure** alias (P1 `0x400D_8200`, P2 `0x4005_0400`) in `flpr_pingpong.c`, and/or grant the
> FLPR the port via the SPU `PERIPH[n].PERM` ownership the way the M33 build already needs the
> Board-Configurator ext-mem-off / 3.3 V VDDM settings. Record the answer here when the board
> confirms it.

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
(C, `flpr_pingpong.c`) — both static-assert the 64-byte size. `#[repr(C)]` + all-`u32` members ⇒ no
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

## F2 — one source sub-line

F2 adds the **single most timing-critical piece of the epic**, isolated: the FLPR clocks out **one
source sub-line** from a write buffer. The blob is now `src/flpr/flpr_source.c` (successor to F1's
`flpr_comms.c`) — same control block + doorbells, plus a `CMD_SHIFT_SUBLINE` that drains `buf[0]`.
No gate scan, no `INTB` frame, no COM, no glass: just the inner data-shift loop, bring-up-slow and
LA-diffed against the M33 `PanelBus` (`src/ls021.rs`), which #139 already proved correct.

### Write-buffer format v0

One sub-line is **`BCK_PER_SUBLINE = 124` u32 words** (120 data columns + 4 trailing dummy/flush,
matching the datasheet horizontal chart and `PanelBus`). Each word holds the **6 data bits already
shifted to their P2 pin positions** — `bit0 R0 … bit5 B1` = `DATA_MASK 0x3F` (the [pin
table](#pin-ownership--the-shared-p2-rule)); `BCK` is *not* in the word (it's the FLPR's own pulse).
The 4 dummy words are `0` (black). So the FLPR inner loop is bit-twiddle-free — *store → pulse BCK →
next* (the issue's explicit goal):

```c
data = buf[col] & 0x3F;          // 6 data bits in P2.00..05 positions
P2.OUTCLR = (~data) & 0x3F;      // lower the 0 bits  ── present the column
P2.OUTSET = data;                // raise the 1 bits  ──  (one xori, no 2nd word)
busy(DATA_SETUP_ITERS);          // data setup before BCK rises
P2.OUTSET = 1<<6;                // BCK high (latches the pixel-pair into the source SR)
if (col == 0) P1.OUTCLR = 1<<7;  // BCK(1) rose within BSP high → release BSP
busy(BCK_HALF_ITERS);
P2.OUTCLR = 1<<6;                // BCK low
busy(BCK_HALF_ITERS);
```

> **Why a set/clear word, not a raw `OUT` write.** Presenting a column is "make `P2.00..05` equal
> these 6 bits." A single `P2.OUT = word` would do it in one store — but it would also clobber the
> M33's `COM` pins (`P2.07/08/10`) and LED0, breaking the [shared-P2 rule](#pin-ownership--the-shared-p2-rule).
> `OUTCLR (~w & 0x3F)` then `OUTSET (w & 0x3F)` touches only the 6 data bits, atomically; the
> complement is one on-the-fly `xori`, so the format still needs just **one word per column**.

The buffer lives in the **SHARED page** (`WRITE_BUF_ADDR = 0x2003_F100`, clear of the 64-byte control
block at the page base) — the region both cores already reach (the FLPR reads the control block
there; the epic's diagram puts the write buffers in shared RAM). The M33 publishes it through the
**`buf[0]` descriptor** (`ptr`/`len`/`ready`) and the FLPR reads `ptr`/`len` *from the descriptor*,
not a hardcoded address — exactly the F4 ping-pong handshake, exercised now with a single buffer.

### The source-shift loop (FLPR) & ack

`drive_subline()` mirrors `PanelBus::shift_subline_with`: `BSP` high → for each of `len` columns,
the snippet above → leave the data lines Lo. `BSP` (P1.07) is raised once before the loop and
released on the **first** `BCK` rising edge so `BCK(1)` falls within `BSP` high (the chart). Then the
FLPR acks like F1 — echo `buf[0].ready → buf[0].consumed`, `status = columns driven`, `flpr_seq =
seq` (seq last, fenced), poke EGU20 — and toggles LED0 once as a by-eye "serviced" marker (after the
ack, so it perturbs neither the round-trip nor the captured waveform). The M33 checks `consumed ==
ready && status == 124 && flpr_seq == seq`.

### Timing — bring-up-slow & the BCK budget

The delays are FLPR busy-loops (`BCK_HALF_ITERS`, `DATA_SETUP_ITERS`), **LA-calibrated on the
bench** — the FLPR analog of the M33 path's `asm::delay` counts. Target: `BCK` well under the
0.758 MHz spec max so the analyzer resolves every edge (speed is F5's job).

> ⚠️ **The FLPR clock is unconfigured at this stage** (the blob sets no clocks), so the
> iteration-count → wall-time mapping is unknown until measured. This is exactly the issue's open
> question — *can the FLPR toggle the pins fast enough?* — answered **off the analyzer, not by
> assumption** (the L1 "PWM won't route to these pins" lesson).
>
> **Measured (bench, 8 MHz LA):** with `BCK_HALF_ITERS = 120`, `DATA_SETUP_ITERS = 40`, `BCK` runs
> **~45 kHz** (hi 9.4 µs, lo 12.7 µs — the lo phase also absorbs the next column's data-present +
> setup) — comfortably under the 0.758 MHz max and ≫ the 660 ns hi/lo floor. That `busy(120) ≈
> 9.4 µs` ⇒ ~78 ns/iter ⇒ the **unconfigured FLPR runs ≈ 64 MHz** (half the M33's 128 MHz). So the
> FLPR clears the `BCK` budget with enormous headroom even bit-banged and unclocked — F5's job is to
> *speed it up* toward the panel's real ~53 ms frame, not to make it keep up. The answer to "can it
> toggle the pins fast enough" is an emphatic yes.

### Verification (LA diff vs the M33 golden sub-line) — ✅ DONE on the bench

`cargo run --release --bin ls021_flpr_bringup --features ls021-flpr`:

- **RTT (verified):** `FLPR alive`, then `sub-line N/5 OK — drove 124 BCK, consumed=0xBEEF000N` for
  `N=1..=5`, then loops forever. (`status = 124` is the FLPR's returned column count; `consumed`
  echoes `ready`.) A `MISMATCH` dumps `status`/`consumed`/`flpr_seq`; a `TIMEOUT` localizes the
  M33→FLPR vs the EGU-return leg exactly as F1.
- **Logic analyzer (verified, RP2040 sigrok-pico rig):** captured `BSP` (D8), `BCK` (D9), and the 6
  data lines (D10..D15 = `R0/R1/G0/G1/B0/B1`) — a wide 2 MHz/50 ms pass for the count+pattern and an
  8 MHz pass for the edge overlap. All invariants hold:
  - exactly **124 `BCK` per `BSP`** on every sub-line;
  - **`BCK(1)` within `BSP` high** — `BSP` high ~3.4 µs, `BCK(1)` rises at +3.1–3.25 µs, `BSP` falls
    0.12–0.25 µs *after* `BCK(1)` rises (matching the M33 golden's `set BCK` → `clear BSP` order;
    needs the 8 MHz pass to resolve — at 2 MHz the two edges share a sample);
  - `BCK` hi 9.4 µs / lo 12.7 µs (≈45 kHz) — far under the 758 kHz max, ≫ the 660 ns floor;
  - the 6 data lines carry the test pattern **exactly** (`data_mismatch = 0`): column `c`'s word is
    `c & 0x3F`, so each line is a clean **divide-by-2ⁿ** square wave — the captured rise counts cascade
    `R0:180 R1:90 G0:45 G1:22 B0:12 B1:6`. No **bit-swap**, **stuck line**, or **odd/even-interleave**
    error. `BSP` toggling on **P1.07** confirms the FLPR reaches a non-P2 port (the F3 prerequisite).
- **No glass** at this stage — with no gate scan and `INTB` low, nothing latches to a pixel; the
  proof is entirely on the analyzer.

> **Bench note — capturing sparse bursts.** The verification sweep / forever-loop space sub-lines out
> (hundreds of ms / `Timer` apart), and the sigrok-pico `-w` hardware trigger proved unreliable here,
> so the captures above were taken with the forever-loop gap temporarily shortened (`Timer::after_*`)
> so an *untriggered* window always lands on sub-lines — the same untriggered approach L2/L3 used.
> Also: the pico pysigrok driver mis-frames a channel count that's an **exact multiple of 7** (it
> over-reads one byte/sample) — capture **15** channels (`D2..D16`), not 14.

## F3 — full frame (init-black + solid colour)

**The "FLPR drives the panel" milestone.** F3 wraps the F2 source-shift loop in the two pieces that
turn one sub-line into a whole frame on glass — the **gate scan** (`GSP`/`GCK`/`GEN`) and the **frame
envelope** (`INTB`) — so the FLPR now owns the *complete* LS021 waveform. The blob is now
`src/flpr/flpr_frame.c` (successor to F2's `flpr_source.c`); everything is a faithful C port of the
analyzer-verified M33 `PanelBus` (`src/ls021.rs`, epic #139), the golden reference. The M33's only
panel job is to **pack one row buffer** and ring the FLPR once per frame; **COM free-runs on the M33**
the whole time. This is the first stage that puts an FLPR-driven frame on glass.

The two hard-won protocol rules from #143 (see [`hardware/display-protocol.md`](../../docs/content/hardware/display-protocol.md))
carry over verbatim — the FLPR reproduces them, it doesn't change them:

- **`INTB` HIGH for the whole frame** — `INTB` low means "no write" (the panel holds its image), so
  *every* frame, the init-black one included, is enveloped in `INTB` high.
- **`GCK` *level* selects the area block on the SAME gate line** — one pixel row = one `GCK` period:
  the **MSB plane** shifts in the `GCK`-HIGH phase (latched into the 2/3-area block) and the **LSB
  plane** in the `GCK`-LOW phase (the 1/3-area block), a `GEN` pulse latching each. 320 gate lines,
  one gate advance per row.

### Gate-line pins (all on P1)

F2 proved the FLPR can drive a non-P2 (P1) GPIO via `BSP` (P1.07) — exactly the thing the gate scan
depends on. The four gate lines join `BSP` on **P1** (all µs-scale, so P2's fast trace domain stays
reserved for the source bus); the harness map is the [bring-up doc](ls021-bringup.md#pinout-21-pin-fpc--dk-pin-map):

| line | DK pin | port.bit | mask | role |
|---|---|---|---|---|
| `GSP` | P1.11 | bit 11 | `1<<11` | gate start pulse (once per frame) |
| `GCK` | P1.12 | bit 12 | `1<<12` | gate clock — HIGH = MSB/2-3 phase, LOW = LSB/1-3 phase |
| `GEN` | P1.04 | bit 4 | `1<<4` | gate output enable — latches the GCK-level-selected block |
| `INTB` | P1.06 | bit 6 | `1<<6` | frame envelope — HIGH for the whole frame write |

> **Both cores drive the shared P2 port at once — new in F3.** Until now COM never ran while the FLPR
> drove the source bus. F3 starts COM (`VCOM`/`VB`/`VA` = P2.07/08/10, **M33**-driven) right after the
> init-black frame, so the M33's COM set/clears and the FLPR's source set/clears (`P2.00..06`) hit the
> **same** P2 port concurrently. This is exactly the case the epic's atomic-`OUTSET`/`OUTCLR` rule was
> written for: disjoint pin masks, no read-modify-write of `OUT`, so the two cores never corrupt each
> other's pins. The gate lines on P1 are FLPR-only; the M33's LED1 heartbeat (P1.10) is disjoint.

### Write-buffer format v1 — a ROW (two sub-lines)

F2's buffer was one sub-line (124 words). F3's is one **row** = `2 × BCK_PER_SUBLINE = 248` u32 words:
the **MSB sub-line** at `[0..124)` then the **LSB sub-line** at `[124..248)`, each in the F2 word
format (`bit0 R0 … bit5 B1`, `BCK` is the FLPR's own pulse). The M33 publishes it through the `buf[0]`
descriptor with **`len = BCK_PER_SUBLINE` (124, the per-sub-line count)**; the FLPR reads the MSB
sub-line from `ptr[0..len)` and the LSB from `ptr[len..2·len)`. The buffer still lives in the SHARED
page (`WRITE_BUF_ADDR = 0x2003_F100`, 992 B, clear of the 64-byte control block).

For a solid colour every row is identical, so the FLPR **reuses the one buffer for all 320 rows** — no
per-row refill. That keeps F3 a "single write buffer" stage (the issue's framing) while still being
**forward-compatible with F4**: a row buffer is exactly the unit F4 ping-pongs (`buf[0]`/`buf[1]`
alternating per row so the M33 can pack row N+1 while the FLPR scans row N). The RGB222→wire **pack**
is a small Rust fn on the M33 (`pack_solid` / `fill_solid_buffer` in the launcher) — the host-tested
seam the epic reserves; F3's is the uniform stand-in, F4 grows it into the real framebuffer pack.

### The frame command & the gate-scan port (FLPR)

One new command, `CMD_RUN_FRAME = 2` (the F2 `CMD_SHIFT_SUBLINE = 1` is subsumed). On it the FLPR runs
`run_frame(buf[0].ptr, buf[0].len)`, a line-for-line port of `PanelBus::fill_solid`:

```
INTB high → FRAME_SETUP → GSP high → FRAME_SETUP
2 lead dummy advances (the first releases GSP on its GCK edge)
320 rows, each one GCK period:
    GCK high → GCK_SETTLE → drive MSB sub-line (BSP + 124 BCK) → GEN pulse   (latch 2/3-area)
    GCK low  → GCK_SETTLE → drive LSB sub-line (BSP + 124 BCK) → GEN pulse   (latch 1/3-area)
6 trail dummy advances
GSP low → INTB low        (panel now holds the image)
```

`drive_subline` is the unchanged F2 loop; `gen_pulse`/`dummy_advance`/`write_gate_row` are ports of the
matching `PanelBus` methods. The FLPR then acks like F2 — echo `buf[0].ready → consumed`, bump
`frame_count`, `status = rows scanned (320)`, `flpr_seq = seq` (seq last, fenced), poke EGU20 — and
blinks LED0 once per drained frame. The M33 checks `consumed == ready && status == 320 && flpr_seq ==
seq`, with a generous 5 s ack timeout (a bring-up-slow frame is ~1.8 s; COM keeps toggling on its
interrupt executor while the M33 awaits).

### Timing — the gate delays

The source-shift counts keep their analyzer-verified F2 values (`BCK_HALF_ITERS = 120`,
`DATA_SETUP_ITERS = 40` ⇒ `BCK ≈ 45 kHz`). The new gate-scan delays are derived from the F2 bench
calibration `ITERS_PER_US ≈ 13` (`busy(120) ≈ 9.4 µs` on the unconfigured ~64 MHz FLPR), the way the
M33 path derives its delays from `COUNTS_PER_US` — each clears its datasheet minimum with margin:

| delay | iters | ≈ µs | datasheet floor |
|---|---|---|---|
| `GCK_SETTLE` | `5·13 = 65` | ~5 | settle after a GCK level change |
| `GEN_SETUP` (setup *and* hold) | `17·13 = 221` | ~17 | `GCK`↔`GEN` ≥16.37 µs |
| `GEN_HIGH` | `25·13 = 325` | ~25 | `GEN` valid-output ≥24.56 µs |
| `GCK_HIGH` (dummy advance) | `10·13 = 130` | ~10 | — |
| `FRAME_SETUP` | `10·13 = 130` | ~10 | `INTB`→`GSP`, `GSP`→first `GCK` |

A full frame ≈ 320 rows × 2 sub-lines × 124 `BCK` × ~22 µs ≈ **1.8 s** — bring-up-slow on purpose (LA-
resolvable). F5 tunes toward the panel's real ~53 ms frame.

### Verification — ✅ DONE on glass

`cargo run --release --bin ls021_flpr_bringup --features ls021-flpr`:

- **On glass (verified):** uniform **black** init frame holds, then **BTN0 steps clean solid white /
  R / G / B** across the whole panel — **identical to the M33-direct L3 (#143)** colour cycle, now
  FLPR-driven. Since L3 is the analyzer-verified golden reference and F3 reproduces it pixel-for-pixel
  on the same glass, that on-glass match is the end-to-end proof the FLPR owns the complete waveform
  correctly (gate scan + `INTB` envelope + source shift) — no separate LA capture was needed. COM
  free-running on the M33 alongside the FLPR's source bus on the shared P2 port showed no interference.
- **RTT:** `FLPR alive`, then `INIT-BLACK frame OK — FLPR scanned 320 rows (frame #1)`, `COM RUNNING`,
  then `WHITE/RED/GREEN/BLUE frame OK` as BTN0 steps (a `MISMATCH` would dump
  `status`/`consumed`/`flpr_seq`; a `TIMEOUT` localizes the M33→FLPR vs the scan-stall vs the
  EGU-return leg).
- **If a future change needs the LA golden-diff** (e.g. F5's speed-tune), capture the gate lines
  (`GSP`/`GCK`/`GEN`/`INTB`) + the source bus (`BSP`/`BCK`/`R0..B1`) and assert the same invariants the
  M33 `PanelBus` passes in #143: `GSP` ×1 with `GCK(1)` within `GSP` high; the right `GCK` count (320
  data periods + lead/trail dummies); `GEN` per phase (≥24.56 µs hi); `GCK`↔`GEN` setup/hold ≥16.37 µs;
  124 `BCK`/sub-line, two sub-lines/row; `INTB` high for the whole frame. The L2/L3 `*_check.py`
  helpers apply unchanged.
- **⚠️ meter `VDD2` first** if a scan ever looks perfect on the LA but the glass stays garbage — the
  #142 loose-5 V-rail gotcha (the gate driver is invisible to the LA).

## F4 — ping-pong from the framebuffer

**The epic's headline deliverable.** F4 keeps the *complete* waveform F3 built — the gate scan, the
`INTB` envelope, the source shift, the two protocol rules — and changes exactly one thing: where each
pixel row's source words come from. F3 packed one solid-colour row buffer and reused it for all 320
rows; F4 sources the rows from a **real 75 KB RGB222 framebuffer**, packs them **one at a time**
through a host-tested Rust fn, and feeds them to the FLPR over **two ping-pong write buffers** so the
M33 packs row N+1 while the FLPR scans row N. That makes the source spatially-varying (the 64-colour
palette, the shapes card) on a fixed two-buffer footprint. The blob is now `src/flpr/flpr_pingpong.c`
(successor to F3's `flpr_frame.c`).

### The ping-pong unit

**One gate line = one row buffer = the MSB sub-line + the LSB sub-line** (`ROW_WORDS = 248` u32). The
M33 packs a whole row into one buffer and rings; the FLPR drains that whole row (both area planes)
before swapping. `buf[0]` carries the **even** rows, `buf[1]` the **odd** — they alternate by
`row & 1`, so each is refilled while the other is scanned.

> **Why the row, not the sub-line.** A single-sub-line unit (swap between the MSB and LSB plane of
> the *same* row) is possible and halves the buffer, but it doubles the handshake traffic and splits
> a single physical gate-line write across two producer/consumer round-trips for no real RAM win (a
> row buffer is < 1 KB). The row is the natural atom: it matches `PanelBus::write_gate_line`, the
> FLPR's `drain_row`, and the eventual partial-update unit (a *changed row*). Two row buffers (≈ 2 KB)
> sit comfortably in the 4 KB `SHARED` page beside the 64-byte control block.

### Write-buffer format v2 — two ROW buffers

F3 had one row buffer; F4 has **two**, at `WRITE_BUF_ADDR = [0x2003_F100, 0x2003_F100 + ROW_WORDS·4]`,
each in the F3 row layout (MSB sub-line `[0..124)` then LSB sub-line `[124..248)`, the F2 word format
`bit0 R0 … bit5 B1`). The M33 publishes both through the `buf[0]`/`buf[1]` descriptors with `len =
BCK_PER_SUBLINE` (124); the FLPR reads each `buf[i].ptr`/`len` from its descriptor. The word format
itself is **unchanged** from F2/F3 — only the *source* of the words (the framebuffer pack) and the
*count* of buffers (two, ping-ponged) are new.

### The pack fn — host-tested Rust, off the C blob

The trickiest piece — turning RGB222 pixels into the panel's MSB/LSB-split, odd/even-interleaved,
pre-shifted GPIO words — is deliberately **not** in the C blob. It is one pure Rust fn,
[`obc_platform::ls021_pack_row`](../obc-platform/src/ls021_wire.rs), unit-tested on the host and run
in CI (the board crate has no test harness, so this lives in the shared `obc-platform` workspace
crate — the sibling of `device64_to_rgb565`, the ST7789's expand). It packs one row of
[`FbDevice64`](../obc-platform/src/framebuffer.rs) device-64 bytes (`0b00_RR_GG_BB`) into the 248
write-buffer words:

- **area-gradation split** — each channel's 2-bit level → the MSB plane's 2/3-area bit (`level >> 1`)
  and the LSB plane's 1/3-area bit (`level & 1`), exactly `PanelBus::plane_bits`;
- **odd/even column interleave** — each `BCK` clocks a pixel *pair*: the even-`x` pixel on `R0/G0/B0`
  (bits 0/2/4), the odd-`x` pixel on `R1/G1/B1` (bits 1/3/5);
- **pre-shift** — the 6 bits land already at their P2 GPIO positions (`= DATA_MASK 0x3F`), so the
  FLPR inner loop stays the bit-twiddle-free `store → pulse BCK` from F2;
- **4 trailing dummy/flush columns** per sub-line = black.

The tests assert byte-for-byte agreement against an independent longhand re-derivation of
`plane_bits` + the `R0..B1` bit positions across a spatial pattern, and pin the catch cases (R/G/B
swap, odd/even interleave, the level-2 MSB-only / level-1 LSB-only split, black → all-zero). The C
blob never packs a pixel — it only drains pre-packed words, so the format can't drift untested.

### The framebuffer source

The M33 holds a resident **`FbDevice64`-format 75 KB `.bss` framebuffer** (240×320 device-64 bytes) —
the production map plane's exact type and size. F4 fills it with the **shared** `palette` / `shapes`
pattern fns (moved into `src/ls021.rs` so the M33-direct L3 path and the FLPR path render the *same*
source — that identity is the on-glass check), then packs one row of it per ping-pong buffer fill.
F5 swaps the pattern fill for the real `App` render behind the `Panel` seam; the pack + ping-pong
path is unchanged.

### The handshake — back-pressure both ways

Each `buf[i]` carries two counters, `ready` (M33) and `consumed` (FLPR):

| step | actor | action |
|---|---|---|
| fill | M33 | pack a fresh row into `buf[i]`, `dsb`, then `ready += 1` (data before the guard) |
| drain | FLPR | wait `ready != consumed`, `fence`, scan the gate row, then `consumed = ready` (`fence`) |

- **FLPR back-pressure:** it waits for `ready != consumed` before scanning, so it never reads a
  half-filled buffer. The wait sits *before* the gate row with `GCK` low, so even a (never-observed)
  stall just holds the inter-row gap with `INTB` high and no `GEN` — nothing latches.
- **M33 back-pressure:** it waits for `consumed == ready` before refilling, so it never overwrites a
  buffer the FLPR is mid-scan on.

A frame is bracketed as before: the M33 resets both descriptors, pre-packs rows 0/1, then writes
`cmd = CMD_RUN_FRAME` + bumps `m33_seq` (the per-frame **command** doorbell, one `dsb` ordering the
whole pre-fill); the FLPR runs the gate scan draining `buf[row & 1]` per row; at frame end it bumps
`frame_count`, sets `status = 320` + `flpr_seq = m33_seq`, and pokes `EGU20` (the FLPR→M33 frame-done
doorbell, IRQ #201). The M33 checks `status == 320 && flpr_seq == m33_seq`.

### Timing — the pack-vs-drain overlap

At bring-up `BCK` the FLPR drains a row in ~5.6 ms (a frame is ~1.8 s) while the M33 packs one in a
handful of µs, so the M33 races far ahead and spends the frame mostly *waiting* on the FLPR — which is
the whole point: the pipeline overlaps, the M33 is never the bottleneck. The launcher logs it per
frame: the FLPR's frame time, the M33's **summed** pack time over the 318 in-frame rows, and the
per-row averages. `pack_total ≪ frame_us` (avg pack µs ≪ avg drain ms) is the RTT proof the issue
asks for.

> **Measured on glass:** FLPR **5663 µs/row → 1.812 s/frame**; M33 pack **105 µs/row avg (124 µs max),
> 33.6 ms summed** over the 318 in-frame rows. So the M33 is active **1.85 %** of the frame and idle
> ~98 % — a **~54× per-row margin** (it could pack 54 rows in the time the FLPR drains one). The
> ping-pong is genuinely concurrent with the depth-2 buffer never the constraint; pack time is flat
> across solids vs the palette/shapes (the pack does fixed per-row work). F5 speeds the FLPR side
> toward the panel's real ~53 ms frame; the M33 already has orders of magnitude of slack.

### Verification — ✅ DONE on glass

- **Host / CI ✅:** `cargo test -p obc-platform` runs the `ls021_wire` pack tests (no hardware);
  `cargo build --release --bin ls021_flpr_bringup --features ls021-flpr` builds the bin + blob, and
  the default `main.rs` build + the workspace `ci` gate stay green.
- **On glass ✅:** `cargo run --release --bin ls021_flpr_bringup --features ls021-flpr` → init-black
  holds, then BTN0 steps **WHITE → R → G → B → 64-colour palette → shapes**, each FLPR-driven from the
  framebuffer over the ping-pong path, **visually identical to the M33-direct L3 (#148)** — the same
  shared `palette`/`shapes` fns render both, so a clean match is the end-to-end proof the pack is
  right (no channel swap, odd/even interleave error, or MSB/LSB mix-up). Since L3 is the
  analyzer-verified golden reference, the on-glass match needed no separate LA capture.
- **RTT ✅:** every frame `frame OK` (`status == 320`, `flpr_seq` matched, `frame_count` 1→7), no
  `MISMATCH`/`TIMEOUT`/`STALLED`, and the overlap line shows **`pack ≪ drain`** with the measured
  margin above (105 µs pack vs 5663 µs drain per row). The fault-localizing error lines were not hit.
- **LA (optional, not needed):** the waveform is byte-identical to F3 (already analyzer-verified), and
  the M33 races ~54× ahead so a buffer swap can't open an inter-row gap — confirmed indirectly by the
  clean glass + the timing margin. Recipe kept for F5's speed-tune (capture `GCK`/`GEN` + the source
  bus across a row boundary).
- **⚠️ meter `VDD2` first** if a future change ever looks perfect on the LA but the glass stays
  garbage (#142).

## F5 — Panel backend + speed-tune

**The bridge to running the app.** F4 proved the FLPR can drive a frame from a resident framebuffer
over the ping-pong path; F5 wraps that push behind the board-agnostic **`obc_platform::Panel`** seam
so the *same* whole-frame generators that drive the ST7789 drive the real LS021, and ratchets the
bit-bang clocks toward the panel's ~53 ms spec frame. The blob, the pack, and the ping-pong handshake
are all **unchanged** from F4 — F5 is M33-side glue plus a timing tune.

### `Ls021Flpr` — the Panel backend

`src/bin/ls021_flpr_bringup.rs` now defines `Ls021Flpr`, an `obc_platform::Panel` impl over the
resident 75 KB RGB222 framebuffer:

- **`band_rows`** — the RGB565 band scratch height (`BAND_ROWS = 16` full-width rows; the frame is
  resident in `FB`, so the band is only a transient per-band draw buffer).
- **`begin_frame`** — a no-op (the plane is filled band-by-band, then driven once).
- **`flush_band(y0, rows, fill)`** — hands the generator a `WIDTH × rows` RGB565 scratch, then
  **quantises it into the resident plane** at rows `[y0, y0+rows)`: each pixel is snapped to the
  device-64 gamut by the host-tested `obc_reader::rgb565_to_device64` (`0/85/170/255 → /85 →` the
  2-bit level) and stored as a `0b00_RR_GG_BB` byte — the same quantiser the glass-demo's swatches
  are drawn from, so a band lands on the panel's gamut exactly as the ST7789 stand-in shows it.
- **`end_frame`** — runs the **whole-frame** FLPR push (the F4 `push_frame`: pre-pack rows 0/1, ring
  `CMD_RUN_FRAME`, pack the rest under the ping-pong handshake, busy-wait the ack).

### Why full-frame push per `end_frame`

The FLPR scans the *whole* frame top-to-bottom in one `CMD_RUN_FRAME` — a band can't reach glass on
its own — so the seam is **full-frame push per `end_frame`**, not a band-incremental feed: `flush_band`
only *fills* the plane, `end_frame` drives all 320 rows once. This is the natural shape for a
scan-the-whole-frame backend and keeps the ping-pong (M33 packs row N+1 while the FLPR scans row N)
exactly as F4 built it. The same generator (`demo::font_palette_demo`, drawn through `Band`) thus
drives the banded ST7789 *and* this full-frame plane unchanged — the proof the seam is panel-agnostic.

### Blocking push (the sync `Panel` seam)

`Panel` is synchronous, so `end_frame` **busy-polls** (`spin_until`) rather than awaiting the F4
EGU20 IRQ: it spins on each `buf[i].consumed == ready` (the M33 is a dedicated packer) and on the
FLPR's `flpr_seq` ack. COM still free-runs on its own high-priority `InterruptExecutor`, so blocking
the thread-mode M33 for a frame is benign — the same shape as the ST7789 path blocking on its SPI-DMA
write. The blob still pokes `EGU20` after each frame; with its IRQ unarmed here that write is a
harmless no-op (the FLPR→M33 doorbell returns when N7 makes the push async behind the app loop).

### Speed-tune — what is safe, and how far

F2–F4 ran deliberately bring-up-slow (`BCK_HALF_ITERS = 120` ≈ 9.4 µs half ⇒ ~53 kHz `BCK`, ~16×
under the panel's 0.758 MHz max) so the analyzer resolved every edge. The **source-shift counts are
the only safe lever**: F5 takes a first conservative step to **half** the F2 value
(`BCK_HALF_ITERS = 60` ≈ 106 kHz `BCK`, still 7× under max — edges stay clean), roughly halving frame
time. On the bench, dial `BCK_HALF_ITERS` down toward **~9** (and `DATA_SETUP_ITERS` toward ~5) while
LA-checking `BCK ≤ 0.758 MHz` with clean edges.

The gate timings (`GCK_SETTLE`/`GEN_SETUP`/`GEN_HIGH`) are panel **electrical minimums** (GCK↔GEN
setup/hold ≥16.37 µs, GEN valid ≥24.56 µs) — **not** slack; do not lower below their µs values.
Because this driver is sequential (gate then source per row), the summed gate time (~38 ms over 320
rows) is the frame-time **floor**: even with `BCK` at max the bit-banged frame lands in the low
hundreds of ms, approaching the ~53 ms spec only as `BCK` nears its ceiling (the spec frame assumes a
pipelined controller that overlaps gate and source — a partial/dirty-line follow-up, deferred). The
`push_frame` RTT line logs the measured frame time each push — tune against that.

### Verification — 🛠️ pending on glass

- **Host / CI ✅:** `cargo build --release --bin ls021_flpr_bringup --features ls021-flpr` builds the
  bin + blob; `cargo clippy` + `cargo fmt --check` clean; the default `main.rs` build is untouched
  (everything stays behind the `ls021-flpr` feature) and `cargo test -p obc-platform` (the pack tests)
  stays green.
- **On glass (pending):** `cargo run --release --bin ls021_flpr_bringup --features ls021-flpr` →
  init-black holds, then BTN0 steps **GLASS-DEMO → WHITE → R → G → B**; the glass-demo card (font
  ladder + 64-colour gamut) must be **visually identical to the ST7789 `--features glass-demo`
  build**, now FLPR-driven via the `Panel` seam.
- **LA (pending):** at each speed step, `BCK ≤ 0.758 MHz` with clean edges; read the per-frame time
  off RTT and off the analyzer. The solids give clean single-value waveforms for the edge check.
- **Docs follow-up:** once on glass, add the FLPR-architecture section to the **public** docs
  (`docs/content/hardware/display-protocol.md` / `software/rendering.md`) as a separate `docs:` commit
  (per the issue), and flip this status to DONE.

## Verification — F1 round-trip

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
